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

#[test]
fn nested_prefab_source_is_refused_and_names_the_inner_sources() {
    let (mut app, root) = open_sample_production("stage-nested");
    // 现造一份"根节点自己又是别的预制体实例"的源（工程里目前没有嵌套样例）。
    let nested_path = root.join("assets/prefabs/nested_slime.prefab");
    fs::write(
        &nested_path,
        "prefab_version=1\nasset_id=22222222-2222-4222-8222-222222222222\nname=NestedSlime\nroot=1\n\
         entity\t1\tNestedSlime\n\
         entity_meta\t1\tprefab.root\ttrue\n\
         entity_meta\t1\tprefab.source\tassets/prefabs/slime_green.prefab\n\
         entity_meta\t1\tprefab.source_id\t1\n",
    )
    .expect("写入嵌套夹具");
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
    }
    let target = entity_id(&app, "SlimeGreen1");
    assert!(matches!(
        app.dispatch(EditorCommand::EnterPrefabStage { entity: target }),
        CommandResult::Rejected(_)
    ));
    assert!(
        app.status.contains("嵌套预制体"),
        "应说明是嵌套挡的：{}",
        app.status
    );
    assert!(
        app.status.contains(GREEN),
        "要把内层源报出来：{}",
        app.status
    );
    assert!(app.prefab_stage().is_none(), "拒绝不得留下半只口袋");
    let _ = fs::remove_dir_all(root);
}
