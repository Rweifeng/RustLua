//! P1-4：预制体 in-context 编辑（对标 Unity 的 Prefab Mode）的回归门。
//!
//! 跑在示例工程 `scenes/api_prefab.scene` 上：它有 **2 个 slime_green 实例**
//! （`SlimeGreen1` / `SlimeGreen2`，各自带 name 与 position 覆盖）和 2 个
//! slime_red 实例。"改源 → 返回 → 两个实例都更新、各自的覆盖还在、别的源不受影响"
//! 这个命题只有真实的多实例夹具才测得动。
//!
//! 实例实体 ID 在每次重建时都会重新分配，所以断言一律**按名字**取实体，
//! 不写死 ID（写死只会制造假失败）。

use super::*;
use crate::SceneViewMode;
use crate::actions::prefab_rebuild::prefab_root_ids;
use editor::{CommandResult, EditorCommand, PlayState};
use rustlua_render::sprite::SpriteComponent;
use rustlua_scene::scene::SceneEntityId;

const GREEN: &str = "assets/prefabs/slime_green.prefab";
const RED: &str = "assets/prefabs/slime_red.prefab";
const GREEN_TINT: [f32; 4] = [0.600, 1.000, 0.600, 1.000];
const RED_TINT: [f32; 4] = [1.000, 0.550, 0.550, 1.000];
/// 阶段里要写进源里的新色调（三个分量都是二进制精确值，逐位比较安全）。
const NEW_TINT: &str = "0.125,0.875,0.25,1";
const NEW_TINT_VALUES: [f32; 4] = [0.125, 0.875, 0.25, 1.0];

/// 当前编辑文档里按名字取实体 ID。
fn entity_id(app: &EditorApp, name: &str) -> SceneEntityId {
    app.document
        .current_scene
        .as_ref()
        .expect("已打开文档")
        .scene
        .entities
        .values()
        .find(|entity| entity.name == name)
        .unwrap_or_else(|| panic!("文档里没有名为 {name} 的实体"))
        .id
}

/// 当前编辑文档里某个实体的 Sprite 色调（解析组件串，不依赖序列化精度）。
fn tint_of(app: &EditorApp, name: &str) -> [f32; 4] {
    let scene = &app
        .document
        .current_scene
        .as_ref()
        .expect("已打开文档")
        .scene;
    let entity = scene
        .entities
        .values()
        .find(|entity| entity.name == name)
        .unwrap_or_else(|| panic!("文档里没有名为 {name} 的实体"));
    SpriteComponent::from_component_str(entity.components.get("Sprite").expect("该实体有 Sprite"))
        .expect("Sprite 组件串应可解析")
        .tint
}

/// 当前编辑文档里某个实体的 Transform 位置分量（覆盖是否仍在就看它）。
fn position_of(app: &EditorApp, name: &str) -> (f32, f32) {
    let scene = &app
        .document
        .current_scene
        .as_ref()
        .expect("已打开文档")
        .scene;
    let entity = scene
        .entities
        .values()
        .find(|entity| entity.name == name)
        .unwrap_or_else(|| panic!("文档里没有名为 {name} 的实体"));
    let text = entity
        .components
        .get("Transform")
        .expect("该实体有 Transform");
    let raw = text
        .split(';')
        .find(|part| part.starts_with("position="))
        .expect("Transform 应有 position")
        .trim_start_matches("position=");
    let mut values = raw.split(',').filter_map(|item| item.parse::<f32>().ok());
    (
        values.next().expect("position.x"),
        values.next().expect("position.y"),
    )
}

/// 场景文档里某个源的全部实例根（按名字排序，便于逐位比较）。
fn instance_names(app: &EditorApp, source: &str) -> Vec<String> {
    let scene = &app
        .document
        .current_scene
        .as_ref()
        .expect("已打开文档")
        .scene;
    let mut names: Vec<String> = scene
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
                    .is_some_and(|value| value == source)
        })
        .map(|entity| entity.name.clone())
        .collect();
    names.sort();
    names
}

/// 进入 `SlimeGreen1` 的 in-context 编辑（各测试的公共前置；失败直接 panic 带原因）。
fn enter_green(app: &mut EditorApp) -> SceneEntityId {
    let target = entity_id(app, "SlimeGreen1");
    assert_eq!(
        app.dispatch(EditorCommand::EnterPrefabStage { entity: target }),
        CommandResult::Applied,
        "应能进入预制体编辑：{}",
        app.status
    );
    assert!(app.prefab_stage().is_some(), "口袋应已建立");
    target
}

#[test]
fn enter_prefab_stage_pockets_the_dirty_scene_and_starts_a_fresh_stack() {
    let (mut app, root) = open_sample_production("stage-enter");
    let hero = entity_id(&app, "Hero");
    // 先在场景里留下**一条真实历史 + 一处未保存改动**：口袋必须把它们一起带走，
    // 否则"不打断"就是谎话。
    assert_eq!(
        app.dispatch(EditorCommand::RenameSelection {
            entity: hero,
            name: "HeroRenamed".to_owned(),
        }),
        CommandResult::Applied
    );
    let scene_entities_before = app
        .document
        .current_scene
        .as_ref()
        .unwrap()
        .scene
        .entities
        .len();
    let undo_before = app.history.undo_len();
    assert!(
        app.document
            .current_scene
            .as_ref()
            .is_some_and(editor::SceneDocument::is_dirty),
        "改名后场景应是脏的"
    );

    enter_green(&mut app);

    // 当前文档已换成源预制体（单实体），且是干净的。
    let stage = app.document.current_scene.as_ref().unwrap();
    assert!(stage.is_prefab_document(), "阶段文档应是预制体文档");
    assert_eq!(
        stage.scene.entities.len(),
        1,
        "slime_green 源只有一个根实体"
    );
    assert!(!stage.is_dirty(), "刚打开的源应是干净的");
    assert_eq!(
        stage
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned()),
        Some("slime_green.prefab".to_owned()),
        "当前文档应是那份 .prefab"
    );
    // 未保存的场景改动与它的撤销栈都在口袋里。
    let pocket = app.prefab_stage().expect("口袋应在");
    assert_eq!(pocket.source, GREEN, "口袋记下来源");
    assert_eq!(pocket.context.scene.entities.len(), scene_entities_before);
    assert!(pocket.context.is_dirty(), "场景的未保存改动必须一起装进去");
    assert_eq!(
        pocket.context_history.undo_len(),
        undo_before,
        "场景的撤销栈应整份进袋"
    );
    assert_eq!(app.history.undo_len(), 0, "阶段内是一条全新的栈");
    assert_eq!(
        app.scene_view_mode,
        SceneViewMode::Prefab,
        "视图应切到预制体模式"
    );
    // 横幅事实：源路径 + 返回时会被同步的实例数（示例场景里 slime_green 有 2 个实例）。
    assert_eq!(
        app.prefab_stage_banner()
            .map(|(source, count)| (source.to_owned(), count)),
        Some((GREEN.to_owned(), 2))
    );
    assert!(
        app.status.contains("正在编辑预制体") && app.status.contains("2 个实例"),
        "状态栏要说清在编辑什么、会动到几个：{}",
        app.status
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn stage_save_and_return_syncs_every_instance_with_one_undo() {
    let (mut app, root) = open_sample_production("stage-save");
    let positions_before = (
        position_of(&app, "SlimeGreen1"),
        position_of(&app, "SlimeGreen2"),
    );
    let undo_before = app.history.undo_len();
    let prefab_path = root.join("assets/prefabs/slime_green.prefab");

    enter_green(&mut app);
    assert_eq!(
        app.dispatch(EditorCommand::SetComponentField {
            entity: entity_id(&app, "SlimeGreen"),
            component: "Sprite".to_owned(),
            field: "tint".to_owned(),
            value: NEW_TINT.to_owned(),
        }),
        CommandResult::Applied,
        "阶段里应能改源的色调：{}",
        app.status
    );
    assert!(app.prefab_stage_unsaved(), "改完源应处于未保存态");

    assert_eq!(
        app.dispatch(EditorCommand::ExitPrefabStage { save: true }),
        CommandResult::Applied,
        "保存并返回应成功：{}",
        app.status
    );
    assert!(app.prefab_stage().is_none(), "退出后口袋必须清空");
    assert!(
        app.status.contains("已写回预制体源并同步场景实例"),
        "状态栏要认领这次同步：{}",
        app.status
    );

    // 1) 源文件真的落盘了（新色调在磁盘上）。
    let saved = Prefab::load(&prefab_path).expect("源预制体应可重读");
    assert_eq!(
        SpriteComponent::from_component_str(
            saved.entities[&saved.root]
                .components
                .get("Sprite")
                .unwrap()
        )
        .expect("可解析")
        .tint,
        NEW_TINT_VALUES,
        "磁盘上的源应已更新"
    );
    // 2) **两个**同源实例都拿到新源，且各自的覆盖仍在。
    assert_eq!(
        instance_names(&app, GREEN),
        vec!["SlimeGreen1", "SlimeGreen2"]
    );
    for name in ["SlimeGreen1", "SlimeGreen2"] {
        assert_eq!(tint_of(&app, name), NEW_TINT_VALUES, "{name} 应同步新色调");
    }
    assert_eq!(
        position_of(&app, "SlimeGreen1"),
        positions_before.0,
        "实例 1 的位置覆盖应保留"
    );
    assert_eq!(
        position_of(&app, "SlimeGreen2"),
        positions_before.1,
        "实例 2 的位置覆盖应保留"
    );
    // 3) 另一个源（slime_red）一个字节都不该被牵连——同步是按源 scoped 的。
    assert_eq!(instance_names(&app, RED), vec!["SlimeRed1", "SlimeRed2"]);
    for name in ["SlimeRed1", "SlimeRed2"] {
        assert_eq!(
            tint_of(&app, name),
            RED_TINT,
            "{name} 不该被 slime_green 的改动波及"
        );
    }
    // 4) 整件事 = **一条** Undo（不是"写文件 + 每个实例各一条"）。
    assert_eq!(
        app.history.undo_len(),
        undo_before + 1,
        "退出预制体编辑只应产生一条历史"
    );

    // 5) 还原：撤销回到旧源（场景侧），但**磁盘上的预制体不回滚**——这是刻意口径，
    //    与既有「写回源并更新实例」一致，代价写进 docs/25 §23。
    assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
    for name in ["SlimeGreen1", "SlimeGreen2"] {
        assert_eq!(tint_of(&app, name), GREEN_TINT, "{name} 撤销后应回到旧色调");
    }
    let still_new = Prefab::load(&prefab_path).expect("源预制体应可重读");
    let saved_tint = SpriteComponent::from_component_str(
        still_new.entities[&still_new.root]
            .components
            .get("Sprite")
            .expect("源根应有 Sprite"),
    )
    .expect("可解析")
    .tint;
    assert_eq!(
        saved_tint, NEW_TINT_VALUES,
        "Undo 只回退场景，不回退磁盘上的源"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn stage_discard_and_return_leaves_the_source_file_byte_identical() {
    let (mut app, root) = open_sample_production("stage-discard");
    let prefab_path = root.join("assets/prefabs/slime_green.prefab");
    let bytes_before = fs::read(&prefab_path).expect("源预制体应可读");
    let hero = entity_id(&app, "Hero");
    assert_eq!(
        app.dispatch(EditorCommand::RenameSelection {
            entity: hero,
            name: "HeroRenamed".to_owned(),
        }),
        CommandResult::Applied
    );
    let mode_before = app.scene_view_mode;
    let undo_before = app.history.undo_len();
    app.selection
        .set_single(hero, editor::SelectionSource::Hierarchy);

    enter_green(&mut app);
    assert_eq!(
        app.dispatch(EditorCommand::SetComponentField {
            entity: entity_id(&app, "SlimeGreen"),
            component: "Sprite".to_owned(),
            field: "tint".to_owned(),
            value: NEW_TINT.to_owned(),
        }),
        CommandResult::Applied
    );

    assert_eq!(
        app.dispatch(EditorCommand::ExitPrefabStage { save: false }),
        CommandResult::Applied
    );
    assert!(
        app.status.contains("源文件未改动"),
        "放弃路径要明说没写盘：{}",
        app.status
    );
    assert_eq!(
        fs::read(&prefab_path).expect("源应仍可读"),
        bytes_before,
        "放弃 = 逐字节不变"
    );
    assert!(app.prefab_stage().is_none());
    // 场景侧一切回到进入前：改动、历史、视图模式、选区。
    assert_eq!(
        tint_of(&app, "SlimeGreen1"),
        GREEN_TINT,
        "阶段里的改动不得渗回场景"
    );
    assert_eq!(app.history.undo_len(), undo_before, "放弃不产生历史");
    assert_eq!(
        entity_id(&app, "HeroRenamed"),
        hero,
        "场景里未保存的改名还在"
    );
    assert_eq!(app.scene_view_mode, mode_before, "视图模式应还原");
    assert_eq!(app.selection.primary(), Some(hero), "选区应还原成进入前");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn enter_prefab_stage_refuses_the_shapes_it_cannot_return_from() {
    // 负控 1：非预制体根（普通实体）——没有源可编。
    let (mut app, root) = open_sample_production("stage-refuse-plain");
    let hero = entity_id(&app, "Hero");
    assert!(matches!(
        app.dispatch(EditorCommand::EnterPrefabStage { entity: hero }),
        CommandResult::Rejected(_)
    ));
    assert!(
        app.status.contains("只有预制体根实例"),
        "应说明为什么：{}",
        app.status
    );
    assert!(app.prefab_stage().is_none(), "拒绝不得留下半只口袋");
    assert_eq!(app.history.undo_len(), 0, "拒绝不得产生历史");

    // 负控 2：当前编辑的文档本身就是一份预制体（资源面板双击打开的那条路）。
    let (mut app, root3) = open_sample_production("stage-refuse-prefab-doc");
    app.open_scene_resource(PathBuf::from(GREEN));
    let inside = entity_id(&app, "SlimeGreen");
    assert!(matches!(
        app.dispatch(EditorCommand::EnterPrefabStage { entity: inside }),
        CommandResult::Rejected(_)
    ));
    assert!(
        app.status.contains("当前编辑的已经是预制体文档"),
        "应给出下一步：{}",
        app.status
    );
    let _ = fs::remove_dir_all(root3);

    // 负控 3：已在阶段里再进一次（会把口袋套娃、第一次的场景就丢了）。
    let (mut app, root4) = open_sample_production("stage-refuse-twice");
    enter_green(&mut app);
    let again = entity_id(&app, "SlimeGreen");
    assert!(matches!(
        app.dispatch(EditorCommand::EnterPrefabStage { entity: again }),
        CommandResult::Rejected(_)
    ));
    assert!(
        app.status.contains("已在预制体编辑上下文中"),
        "应说明先返回：{}",
        app.status
    );
    assert_eq!(
        app.prefab_stage().map(|stage| stage.source.as_str()),
        Some(GREEN),
        "拒绝不该动原来的口袋"
    );
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(root4);
}

#[test]
fn prefab_stage_and_play_are_mutually_exclusive() {
    let (mut app, root) = open_sample_production("stage-play");
    // Play 中不进阶段：阶段会替换当前文档，与运行态镜像撞车。
    app.play.set_state_for_test(PlayState::Playing);
    let green = entity_id(&app, "SlimeGreen1");
    assert!(matches!(
        app.dispatch(EditorCommand::EnterPrefabStage { entity: green }),
        CommandResult::Rejected(_)
    ));
    assert!(
        app.status.contains("播放模式"),
        "应说明是播放模式挡的：{}",
        app.status
    );
    app.play.set_state_for_test(PlayState::Editing);

    // 阶段里不放 Play：跑的是"预制体根当场景根"，与场景里的东西不是一回事。
    enter_green(&mut app);
    assert!(matches!(
        app.dispatch(EditorCommand::StartPlay),
        CommandResult::Rejected(_)
    ));
    assert!(
        app.status.contains("预制体编辑中不能播放"),
        "应给出可执行的下一步：{}",
        app.status
    );
    assert!(app.prefab_stage().is_some(), "拒绝播放不得把阶段拆掉");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn dirty_stage_blocks_document_switch_and_clean_stage_returns_silently() {
    // 脏阶段：换文档会把口袋（连同未保存的源改动与场景）整份丢掉 → 立刻拦住，
    // 而且**不能**先挂出 pending（那时用户已经对着「保存并继续」做了选择）。
    let (mut app, root) = open_sample_production("stage-switch-dirty");
    enter_green(&mut app);
    assert_eq!(
        app.dispatch(EditorCommand::SetComponentField {
            entity: entity_id(&app, "SlimeGreen"),
            component: "Sprite".to_owned(),
            field: "tint".to_owned(),
            value: NEW_TINT.to_owned(),
        }),
        CommandResult::Applied
    );
    app.open_scene_resource(PathBuf::from("scenes/main.scene"));
    assert!(
        app.status.contains("已阻止"),
        "换文档应被拦住：{}",
        app.status
    );
    assert!(
        app.status.contains("保存并返回"),
        "应指回横幅：{}",
        app.status
    );
    assert!(app.pending_document_change.is_none(), "不该留下待确认弹窗");
    assert!(app.prefab_stage().is_some(), "阶段必须还在");
    assert!(
        app.document
            .current_scene
            .as_ref()
            .is_some_and(editor::SceneDocument::is_prefab_document),
        "当前文档应仍是那份预制体"
    );

    // 干净阶段：没东西可丢，静默返回场景后按正常闸门走。
    let (mut app, root2) = open_sample_production("stage-switch-clean");
    enter_green(&mut app);
    app.open_scene_resource(PathBuf::from("scenes/main.scene"));
    assert!(app.prefab_stage().is_none(), "换文档前应已静默返回场景");
    assert_eq!(
        app.document
            .current_scene
            .as_ref()
            .expect("已切换文档")
            .scene
            .name,
        "main",
        "应真的打开另一份场景"
    );
    assert_eq!(
        app.scene_view_mode,
        SceneViewMode::View2D,
        "视图模式不该留在预制体"
    );
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(root2);
}

/// 造出"外层里挂一个真实 `slime_green` 实例"的嵌套预制体并落盘，返回内存里那份。
///
/// 只用引擎自己的 `instantiate` + `apply_prefab_instance_metadata`：手写 TSV 只能测到
/// 我编出来的形状，测不到编辑器真会写出的那份。
fn write_nested_green_prefab(project_root: &Path, relative: &str) -> Prefab {
    write_nested_green_prefab_with_count(project_root, relative, 1)
}

/// 同上，但一次挂 `count` 个内层实例（同一个外层文件里引用同一张内层源多次）。
fn write_nested_green_prefab_with_count(
    project_root: &Path,
    relative: &str,
    count: usize,
) -> Prefab {
    let inner = Prefab::load(project_root.join(GREEN)).expect("内层源可加载");
    let mut holder = rustlua_scene::scene::Scene::new("NestedSlime");
    holder
        .add_entity(SceneEntityId(1), "NestedSlime", None)
        .unwrap();
    for _ in 0..count {
        let staged = inner
            .instantiate(
                &mut holder,
                Some(SceneEntityId(1)),
                &std::collections::BTreeMap::new(),
            )
            .unwrap();
        apply_prefab_instance_metadata(&mut holder, GREEN, &staged);
    }
    let nested = Prefab::from_scene(
        &holder,
        SceneEntityId(1),
        rustlua_assets::resource_db::AssetId::from_seed("nested_slime"),
        "NestedSlime",
    )
    .expect("外层预制体可创建");
    assert_eq!(
        nested.nested_sources(),
        vec![GREEN.to_owned()],
        "外层要记得自己嵌了谁"
    );
    nested
        .save(project_root.join(relative))
        .expect("外层预制体可落盘");
    nested
}

/// 在**当前场景**里实例化这张（嵌套）预制体并盖章。
///
/// 返回 `(外层实例根, 内层 slime_green 实例根)`——后者带 `prefab.outer_id`，是嵌套的身份标记。
fn place_nested_instance(
    app: &mut EditorApp,
    nested: &Prefab,
    relative: &str,
) -> (SceneEntityId, SceneEntityId) {
    let scene = app.document.current_scene.as_mut().expect("已打开场景");
    let instance = nested
        .instantiate(&mut scene.scene, None, &std::collections::BTreeMap::new())
        .expect("嵌套预制体可实例化");
    apply_prefab_instance_metadata(&mut scene.scene, relative, &instance);
    let outer_root = instance.instance_root;
    let inner_root = scene.scene.entities[&outer_root].children[0];
    (outer_root, inner_root)
}

/// **嵌套 Prefab 的 in-context 往返**（docs/55 P1 的 Prefab 一条）。
///
/// 夹具是"外层 `nested_slime` 里挂一个真实的 `slime_green` 实例"（见
/// [`write_nested_green_prefab`]）。要证的命题有三条，都是嵌套最容易被悄悄洗掉的地方：
///
/// 1. 进入阶段不再被"不支持嵌套"挡下（成环与超深仍然拒，见另一个门）；
/// 2. 退出保存后，**内层身份逐字留在外层文件里**（`prefab.source` / `prefab.source_id` /
///    内层实例根标记），场景里重建出的实例也还认得出自己是内层 `slime_green` 的节点——
///    这一步要是丢了标记，那一支就退化成普通节点，UI 再也点不进内层；
/// 3. 在阶段里改**内层节点**的字段只更新外层文件与场景实例，**不碰内层那份 `.prefab`**
///    （改源是"这张外层预制体自己带的快照"，不是替用户改公共资产）。
#[test]
fn nested_prefab_stage_round_trips_the_inner_identity() {
    const NESTED: &str = "assets/prefabs/nested_slime.prefab";
    let (mut app, root) = open_sample_production("stage-nested");
    let nested_path = root.join(NESTED);
    let inner_before = fs::read(root.join(GREEN)).expect("内层源在示例工程里");

    // 1) 内存里搭出"根 + 一个内层实例"，再 save 成工程里的 .prefab（与真创建同一条路）。
    let nested = write_nested_green_prefab(&root, NESTED);

    // 2) 场景里实例化它（= 用户把这张预制体拖进场景）。
    let (outer_root, inner_child_in_scene) = place_nested_instance(&mut app, &nested, NESTED);
    {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let child = &scene.entities[&inner_child_in_scene];
        assert_eq!(
            child.metadata["prefab.source"], GREEN,
            "实例化就得带上内层来源"
        );
        assert_eq!(child.metadata["prefab.source_id"], "1");
        assert_eq!(
            child.metadata["prefab.outer_id"], "2",
            "外层文件里它是 2 号实体"
        );
    }

    // 3) 进入阶段：嵌套不再被拒。
    assert!(
        matches!(
            app.dispatch(EditorCommand::EnterPrefabStage { entity: outer_root }),
            CommandResult::Applied
        ),
        "嵌套预制体现在应能进入 in-context：{}",
        app.status
    );
    assert_eq!(
        app.document
            .current_scene
            .as_ref()
            .expect("阶段文档已打开")
            .scene
            .entities
            .len(),
        2,
        "阶段文档就是那份外层预制体（根 + 内层实例）"
    );

    // 4) 阶段里改**内层节点**的色调（改的是外层自己带的快照）。
    let stage_child = entity_id(&app, "SlimeGreen");
    assert_eq!(
        app.dispatch(EditorCommand::SetComponentField {
            entity: stage_child,
            component: "Sprite".to_owned(),
            field: "tint".to_owned(),
            value: NEW_TINT.to_owned(),
        }),
        CommandResult::Applied,
        "阶段里应能改内层节点的色调：{}",
        app.status
    );

    // 5) 保存并返回。
    assert_eq!(
        app.dispatch(EditorCommand::ExitPrefabStage { save: true }),
        CommandResult::Applied,
        "嵌套的保存并返回应成功：{}",
        app.status
    );
    assert!(app.prefab_stage().is_none(), "退出后口袋必须清空");

    // 6) 磁盘上的外层仍记得内层身份（标记逐字保留）。
    let saved = Prefab::load(&nested_path).expect("外层可重读");
    assert_eq!(
        saved.nested_sources(),
        vec![GREEN.to_owned()],
        "嵌套引用不能洗掉"
    );
    let saved_inner = saved
        .entities
        .values()
        .find(|entity| entity.metadata.contains_key("prefab.source"))
        .expect("外层文件里要有内层标记");
    assert_eq!(saved_inner.metadata["prefab.source"], GREEN);
    assert_eq!(
        saved_inner.metadata["prefab.source_id"], "1",
        "内层文件里的实体编号必须逐字保住"
    );

    // 7) 场景里重建出的实例仍然认得出自己是内层的节点（三个标记都在，且指向内层文件）。
    let scene = app
        .document
        .current_scene
        .as_ref()
        .expect("已返回场景")
        .scene
        .clone();
    let rebuilt_root = prefab_root_ids(&scene, NESTED)
        .into_iter()
        .next()
        .expect("场景里应还有外层实例");
    assert_eq!(
        scene.entities[&rebuilt_root].metadata["prefab.source"],
        NESTED
    );
    let rebuilt_child = scene.entities[&rebuilt_root].children[0];
    let child = &scene.entities[&rebuilt_child];
    assert_eq!(child.metadata["prefab.source"], GREEN, "内层来源要留住");
    assert_eq!(child.metadata["prefab.source_id"], "1", "内层编号要留住");
    assert_eq!(child.metadata["prefab.root"], "true", "它仍是内层实例的根");
    assert!(
        child.metadata.contains_key("prefab.outer_id"),
        "嵌套节点必须知道自己在外层文件里的编号，否则下次重建对不上号：{:?}",
        child.metadata
    );
    // 阶段里的改动同步到了场景实例，而**内层那份公共资产一个字节没动**。
    assert_eq!(
        SpriteComponent::from_component_str(child.components.get("Sprite").expect("有 Sprite"))
            .expect("可解析")
            .tint,
        NEW_TINT_VALUES,
        "实例应拿到阶段里改的色调"
    );
    assert_eq!(
        fs::read(root.join(GREEN)).expect("内层源仍在"),
        inner_before,
        "编辑外层不得改动内层那份公共资产"
    );
    let _ = fs::remove_dir_all(root);
}

/// 从**嵌套节点**进入内层源：改动只落到直接实例，外层那一支的标记与快照都不许被动。
///
/// 这是嵌套最容易悄悄写出坏数据的一条路：如果按内层新源把"嵌在外层里的那一支"也同步一遍，
/// 盖章会把它的 `prefab.outer_id` 洗掉（它会被当成外层自己的直接子节点），外层下次写回
/// 就对不上号、整支被拒。所以 [`prefab_root_ids`] 只认直接实例，内层源的同步**止于**外层边界，
/// 外层里那份仍是它自己的快照（要更新它，用外层实例的「重新应用源预制体」或进外层阶段）。
#[test]
fn editing_the_inner_source_from_a_nested_node_stops_at_the_outer_boundary() {
    const NESTED: &str = "assets/prefabs/nested_slime.prefab";
    let (mut app, root) = open_sample_production("stage-nested-inner");
    let nested = write_nested_green_prefab(&root, NESTED);
    let outer_before = fs::read(root.join(NESTED)).expect("外层已落盘");
    let (_, inner_root_in_scene) = place_nested_instance(&mut app, &nested, NESTED);

    assert!(
        matches!(
            app.dispatch(EditorCommand::EnterPrefabStage {
                entity: inner_root_in_scene
            }),
            CommandResult::Applied
        ),
        "嵌套的内层实例根也应能进入 in-context：{}",
        app.status
    );
    // 状态栏要说清"外层里那份不会被自动更新"，别让人以为场景没变是坏了。
    assert!(
        app.status.contains(NESTED) && app.status.contains("不自动更新"),
        "从嵌套节点进入时要说明外层归属：{}",
        app.status
    );
    assert_eq!(
        app.dispatch(EditorCommand::SetComponentField {
            entity: entity_id(&app, "SlimeGreen"),
            component: "Sprite".to_owned(),
            field: "tint".to_owned(),
            value: NEW_TINT.to_owned(),
        }),
        CommandResult::Applied,
        "阶段里应能改内层源：{}",
        app.status
    );
    assert_eq!(
        app.dispatch(EditorCommand::ExitPrefabStage { save: true }),
        CommandResult::Applied,
        "内层源保存并返回应成功：{}",
        app.status
    );

    // 1) 内层那份公共资产真的写回了新色调。
    let inner_now = Prefab::load(root.join(GREEN)).expect("内层源可重读");
    assert_eq!(
        SpriteComponent::from_component_str(
            inner_now.entities[&inner_now.root]
                .components
                .get("Sprite")
                .expect("有 Sprite")
        )
        .expect("可解析")
        .tint,
        NEW_TINT_VALUES,
        "内层源应拿到新色调"
    );
    // 2) 场景里那两个**直接**实例照常同步（示例夹具就有 SlimeGreen1 / SlimeGreen2）。
    for name in ["SlimeGreen1", "SlimeGreen2"] {
        assert_eq!(tint_of(&app, name), NEW_TINT_VALUES, "{name} 应同步内层源");
    }
    // 3) 外层文件一个字节没动：它带的仍是进入时那份快照（同步止于外层边界）。
    assert_eq!(
        fs::read(root.join(NESTED)).expect("外层仍在"),
        outer_before,
        "改内层源不得改写成外层文件"
    );
    // 4) 嵌在外层里的那一支：色调还是外层快照的老值，**且三个标记一个没被洗**。
    //    按标记定位（不靠名字），免得和场景里别的同名实体认错人。
    let scene = app
        .document
        .current_scene
        .as_ref()
        .expect("已返回场景")
        .scene
        .clone();
    let outer_root = prefab_root_ids(&scene, NESTED)
        .into_iter()
        .next()
        .expect("外层实例还在场景里");
    let nested_node = scene.entities[&outer_root].children[0];
    let markers = &scene.entities[&nested_node].metadata;
    assert_eq!(
        markers.get("prefab.source").map(String::as_str),
        Some(GREEN)
    );
    assert_eq!(
        markers.get("prefab.source_id").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        markers.get("prefab.outer_id").map(String::as_str),
        Some("2"),
        "内层同步若洗掉外层编号，外层下次写回就会整支被拒：{markers:?}"
    );
    assert_eq!(
        SpriteComponent::from_component_str(
            scene.entities[&nested_node]
                .components
                .get("Sprite")
                .expect("有 Sprite")
        )
        .expect("可解析")
        .tint,
        GREEN_TINT,
        "嵌套那份由外层文件持有，不该被内层同步改掉"
    );
    // 5) 外层实例此后仍能被"重新应用源预制体"整支重建——坏数据最直接的后果就是这条会失败。
    assert_eq!(
        app.dispatch(EditorCommand::ReapplyPrefab { entity: outer_root }),
        CommandResult::Applied,
        "外层实例应仍能按外层文件重建：{}",
        app.status
    );
    let _ = fs::remove_dir_all(root);
}

/// **一个外层里引用同一张内层源多次**：三份都得在自己的外层位置上，写回不许被拒。
///
/// 两份内层实例来自同一个文件，`prefab.source` 与 `prefab.source_id` 逐字相同，唯一能把
/// 它们分开的就是"在外层文件里是几号实体"。撞在同一个编号上时，写回的重复检查会直接拒
/// （要么静默丢一支），所以这里既数份数也数编号。
///
/// 夹具刻意混出两种内层节点：外层文件**本来就带着**的那一支（已有外层编号），以及在场景里
/// 新挂进外层分支的两支（还没有外层编号）——真实作者流程正是这样：拖一张预制体进外层实例，
/// 再「保存预制体源」。第二支的形状原本会被自己的编号检查拒掉（"嵌套预制体节点 N 没有
/// 本预制体的实体编号"），控制破坏验过：去掉写回前的编号回填这条门就判红。
#[test]
fn repeated_inner_instances_in_one_outer_get_distinct_envelope_ids() {
    const NESTED: &str = "assets/prefabs/nested_slime.prefab";
    let (mut app, root) = open_sample_production("stage-nested-pair");
    let inner_before = fs::read(root.join(GREEN)).expect("内层源在示例工程里");
    // 外层文件里先只有一支内层实例（编号 2），另外两支由场景临时挂进来。
    let nested = write_nested_green_prefab(&root, NESTED);
    let outer_before = fs::read(root.join(NESTED)).expect("外层已落盘");
    let inner_prefab = Prefab::load(root.join(GREEN)).expect("内层源可加载");
    let (outer_root, first_inner) = place_nested_instance(&mut app, &nested, NESTED);

    // 在同一个外层实例下再挂两支内层实例（= 用户把 slime_green 拖进外层分支两次）。
    for _ in 0..2 {
        let scene = app.document.current_scene.as_mut().expect("已打开场景");
        let extra = inner_prefab
            .instantiate(
                &mut scene.scene,
                Some(outer_root),
                &std::collections::BTreeMap::new(),
            )
            .expect("内层预制体可挂进外层分支");
        apply_prefab_instance_metadata(&mut scene.scene, GREEN, &extra);
    }
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    let inner_nodes = scene.entities[&outer_root].children.clone();
    assert_eq!(inner_nodes.len(), 3, "外层实例下应有 3 支内层实例");
    assert!(inner_nodes.contains(&first_inner));
    for node in &inner_nodes {
        let entity = &scene.entities[node];
        assert_eq!(
            entity.metadata["prefab.source"], GREEN,
            "{node:?} 应认得内层源"
        );
        assert_eq!(entity.metadata["prefab.source_id"], "1");
    }
    assert_eq!(
        scene.entities[&first_inner].metadata["prefab.outer_id"], "2",
        "外层文件里那一支的编号要保住"
    );
    // 已在文件里的那支是嵌套节点，不是内层源的"直接实例"；另外两支还没有外层编号，
    // 在写回前确实暂时算直接实例（外层文件还不知道它们的存在）——正因为如此，
    // 写回必须给它们各分配一个编号，而不是把它们当自己的节点或干脆拒掉。
    let direct_greens = prefab_root_ids(scene, GREEN);
    assert!(
        !direct_greens.contains(&first_inner),
        "嵌套节点 {first_inner:?} 不该被算作内层源的直接实例：{direct_greens:?}"
    );

    // 「保存预制体源」：整支外层写回，3 份内层要拿到 3 个互不相同的外层编号。
    assert_eq!(
        app.dispatch(EditorCommand::SavePrefabToSource { entity: outer_root }),
        CommandResult::Applied,
        "重复内层引用应能整体写回：{}",
        app.status
    );
    assert!(
        app.status.contains("已写回预制体源"),
        "写回不该被自己的编号检查拒掉：{}",
        app.status
    );

    // 磁盘上的外层：3 个节点都带内层标记，且它们在文件里的编号互不相同、都不等于文件根。
    let saved = Prefab::load(root.join(NESTED)).expect("外层可重读");
    assert_eq!(
        saved.nested_sources(),
        vec![GREEN.to_owned()],
        "多次引用同一张内层源仍只算一条嵌套引用"
    );
    let saved_ids = saved
        .entities
        .values()
        .filter(|entity| entity.metadata.get("prefab.source").map(String::as_str) == Some(GREEN))
        .map(|entity| entity.id)
        .collect::<Vec<_>>();
    assert_eq!(
        saved_ids.len(),
        3,
        "3 支内层实例都要写进外层文件：{saved_ids:?}"
    );
    assert_eq!(
        saved_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3,
        "外层文件里两份内层实例撞在同一个实体编号上：{saved_ids:?}"
    );
    assert!(
        !saved_ids.contains(&saved.root),
        "文件根属于外层自己，不能同时是内层实例：{:?}",
        saved.root
    );
    // 内层那份公共资产一个字节没动（写回只改外层文件）。
    assert_eq!(
        fs::read(root.join(GREEN)).expect("内层源仍在"),
        inner_before,
        "写回外层不得改动内层公共资产"
    );

    // 场景里重建回来的外层实例：3 个孩子仍各带自己的外层编号，且互不相同。
    let scene = app
        .document
        .current_scene
        .as_ref()
        .expect("已返回场景")
        .scene
        .clone();
    let rebuilt_root = prefab_root_ids(&scene, NESTED)
        .into_iter()
        .next()
        .expect("外层实例还在场景里");
    let rebuilt_children = scene.entities[&rebuilt_root].children.clone();
    assert_eq!(
        rebuilt_children.len(),
        3,
        "重建后的外层实例应仍有 3 支内层实例"
    );
    let outer_ids = rebuilt_children
        .iter()
        .map(|child| scene.entities[child].metadata["prefab.outer_id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        outer_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3,
        "重建后有两份内层实例认出了同一个外层位置：{outer_ids:?}"
    );
    // 写回后 3 支都拿到了外层编号 ⇒ 它们都不再是内层源的"直接实例"：此后改 `slime_green`
    // 公共资产只动场景里那两张直接实例，外层里这三支仍归外层文件管。
    let direct_after = prefab_root_ids(&scene, GREEN);
    for child in &rebuilt_children {
        assert!(
            !direct_after.contains(child),
            "已写回的嵌套节点 {child:?} 不该被算作内层源的直接实例：{direct_after:?}"
        );
    }
    for child in &rebuilt_children {
        assert_eq!(
            scene.entities[child].metadata["prefab.source"], GREEN,
            "重建后仍要认得内层源"
        );
    }
    // 内层没被误同步：3 支的色调都还是外层文件里的快照值。
    for child in &rebuilt_children {
        assert_eq!(
            SpriteComponent::from_component_str(
                scene.entities[child]
                    .components
                    .get("Sprite")
                    .expect("有 Sprite")
            )
            .expect("可解析")
            .tint,
            GREEN_TINT,
            "写回外层不该改内层源的取值"
        );
    }
    // 此后仍能整体重建（坏数据最直接的后果就是这条会失败），且外层文件确实换了内容。
    let reapply = app.dispatch(EditorCommand::ReapplyPrefab {
        entity: rebuilt_root,
    });
    assert!(
        !matches!(reapply, CommandResult::Rejected(_)),
        "多次嵌套后仍应能重新应用源预制体：{}",
        app.status
    );
    assert!(
        app.status.contains("已重新应用预制体"),
        "重新应用应走完整条重建路径：{}",
        app.status
    );
    assert_ne!(
        fs::read(root.join(NESTED)).expect("外层仍在"),
        outer_before,
        "写回应真的改掉外层文件"
    );
    let _ = fs::remove_dir_all(root);
}

/// 嵌套**只拒坏数据**：新预制体引用自己（A 嵌 A）与内层链成环都要给出原因。
#[test]
fn nested_prefab_cycle_is_refused_with_the_cycle_path() {
    let (mut app, root) = open_sample_production("stage-nested-cycle");
    let nested_path = root.join("assets/prefabs/nested_slime.prefab");
    // 手写一份"A 的文件根里挂着 A 自己"的夹具（引擎的创建路径已经拒这种，见 hierarchy 的门）。
    fs::write(
        &nested_path,
        "prefab_version=1\nasset_id=22222222-2222-4222-8222-222222222222\nname=NestedSlime\nroot=1\n\
         entity\t1\tNestedSlime\n\
         entity\t2\tInner\t1\n\
         entity_meta\t2\tprefab.root\ttrue\n\
         entity_meta\t2\tprefab.source\tassets/prefabs/nested_slime.prefab\n\
         entity_meta\t2\tprefab.source_id\t1\n",
    )
    .expect("写入自指夹具");
    {
        let scene = app.document.current_scene.as_mut().expect("已打开场景");
        let target = scene
            .scene
            .entities
            .values_mut()
            .find(|entity| entity.name == "SlimeGreen1")
            .expect("SlimeGreen1");
        target.metadata.insert(
            "prefab.source".to_owned(),
            "assets/prefabs/nested_slime.prefab".to_owned(),
        );
        target
            .metadata
            .insert("prefab.source_id".to_owned(), "1".to_owned());
    }
    let target = entity_id(&app, "SlimeGreen1");
    assert!(matches!(
        app.dispatch(EditorCommand::EnterPrefabStage { entity: target }),
        CommandResult::Rejected(_)
    ));
    assert!(
        app.status.contains("环"),
        "应说明是引用环挡的：{}",
        app.status
    );
    assert!(
        app.status.contains(GREEN) || app.status.contains("nested_slime"),
        "要把成环的那条路径报出来：{}",
        app.status
    );
    assert!(app.prefab_stage().is_none(), "拒绝不得留下半只口袋");
    let _ = fs::remove_dir_all(root);
}
