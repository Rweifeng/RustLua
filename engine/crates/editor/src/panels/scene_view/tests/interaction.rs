//! 指针状态机：按下选中、附加选择、拖框、拖动相位与松手提交。
//! 这一层不碰 GPU，也不进 egui 帧——直接给实体包围框和指针位置就断言相位与副作用。

use super::*;

/// 按在实体上 → 拖动相位 + 单选中该实体 + 记录拖动载荷。
#[test]
fn pointer_down_on_entity_starts_drag_with_selection() {
    let mut app = interaction_app();
    let entities = vec![
        interaction_entity(7, egui::pos2(100.0, 100.0)),
        interaction_entity(8, egui::pos2(300.0, 300.0)),
    ];
    let phase = app.scene_pointer_down(egui::pos2(100.0, 100.0), &entities, false);
    assert!(matches!(phase, ToolPhase::Dragging { .. }), "{phase:?}");
    assert_eq!(app.selection.primary(), Some(SceneEntityId(7)));
    let drag = app.scene_state.drag.as_ref().expect("拖动载荷");
    assert_eq!(drag.start, egui::pos2(100.0, 100.0));
    assert_eq!(drag.ids, vec![SceneEntityId(7)]);
}

/// 按在空白处 → 框选相位 + 清空选择（且不产生拖动载荷）。
#[test]
fn pointer_down_on_empty_space_starts_marquee_and_clears_selection() {
    let mut app = interaction_app();
    app.selection
        .set_single(SceneEntityId(7), editor::SelectionSource::Hierarchy);
    let entities = vec![interaction_entity(7, egui::pos2(100.0, 100.0))];
    let phase = app.scene_pointer_down(egui::pos2(500.0, 500.0), &entities, false);
    assert!(phase.affects_selection(), "{phase:?}");
    assert!(app.selection.is_empty(), "非加选框选必须先清空选择");
    assert!(app.scene_state.drag.is_none());
}

/// 加选（Ctrl/Shift）按在实体上 → 选择变多而不是被替换。
#[test]
fn additive_pointer_down_adds_to_selection() {
    let mut app = interaction_app();
    app.selection
        .set_single(SceneEntityId(7), editor::SelectionSource::Hierarchy);
    let entities = vec![
        interaction_entity(7, egui::pos2(100.0, 100.0)),
        interaction_entity(8, egui::pos2(300.0, 300.0)),
    ];
    app.scene_pointer_down(egui::pos2(300.0, 300.0), &entities, true);
    let mut ids = app.selection.ids();
    ids.sort_unstable();
    assert_eq!(ids, vec![SceneEntityId(7), SceneEntityId(8)]);
}

/// 拖动推进相位里的 `current`（框选矩形的另一端），释放后回到 Idle 且清掉载荷。
#[test]
fn pointer_drag_updates_phase_and_release_returns_to_idle() {
    let mut app = interaction_app();
    let entities = vec![interaction_entity(7, egui::pos2(100.0, 100.0))];
    app.scene_pointer_down(egui::pos2(100.0, 100.0), &entities, false);
    let phase = app.scene_pointer_drag(egui::pos2(140.0, 160.0));
    assert_eq!(
        phase,
        ToolPhase::Dragging {
            start: [100.0, 100.0],
            current: [140.0, 160.0]
        }
    );
    let phase = app.scene_pointer_up(egui::pos2(140.0, 160.0));
    assert_eq!(phase, ToolPhase::Idle);
    assert!(app.scene_state.drag.is_none(), "释放后不得残留拖动载荷");
}

#[test]
fn pick_scene_2d_entity_uses_topmost_unlocked_hit() {
    let transform = TransformFields::default();
    let entities = vec![
        SceneBoxedEntity {
            id: SceneEntityId(1),
            name: "back".to_owned(),
            transform,
            center: egui::pos2(100.0, 100.0),
            size: egui::vec2(80.0, 80.0),
            ui: None,
        },
        SceneBoxedEntity {
            id: SceneEntityId(2),
            name: "front".to_owned(),
            transform,
            center: egui::pos2(100.0, 100.0),
            size: egui::vec2(40.0, 40.0),
            ui: None,
        },
    ];
    let locked = std::collections::BTreeSet::new();
    assert_eq!(
        pick_scene_2d_entity(&entities, egui::pos2(100.0, 100.0), &locked),
        Some(SceneEntityId(2))
    );
    let mut locked = std::collections::BTreeSet::new();
    locked.insert(SceneEntityId(2));
    assert_eq!(
        pick_scene_2d_entity(&entities, egui::pos2(100.0, 100.0), &locked),
        Some(SceneEntityId(1))
    );
}

#[test]
fn box_selection_uses_directional_containment_rules() {
    let transform = TransformFields::default();
    let entities = vec![
        SceneBoxedEntity {
            id: SceneEntityId(1),
            name: "contained".to_owned(),
            transform,
            center: egui::pos2(20.0, 20.0),
            size: egui::vec2(10.0, 10.0),
            ui: None,
        },
        SceneBoxedEntity {
            id: SceneEntityId(2),
            name: "partial".to_owned(),
            transform,
            center: egui::pos2(40.0, 20.0),
            size: egui::vec2(20.0, 20.0),
            ui: None,
        },
    ];
    let left_to_right = egui::Rect::from_min_max(egui::pos2(10.0, 10.0), egui::pos2(25.0, 25.0));
    assert_eq!(
        entities_in_selection_rect(
            &entities,
            left_to_right,
            egui::pos2(10.0, 10.0),
            egui::pos2(25.0, 25.0)
        ),
        vec![SceneEntityId(1)]
    );
    let right_to_left = egui::Rect::from_min_max(egui::pos2(25.0, 10.0), egui::pos2(45.0, 30.0));
    assert_eq!(
        entities_in_selection_rect(
            &entities,
            right_to_left,
            egui::pos2(45.0, 30.0),
            egui::pos2(25.0, 10.0)
        ),
        vec![SceneEntityId(1), SceneEntityId(2)]
    );
}

/// **帧级合成拖动**（docs/57 §4.6 / docs/55 阶段 2「绘制与交互可独立测试」）：
/// 按生产调用序合成 `down → drag(+预览) → drag → up`，断言
/// ① 拖动全过程**不写历史、不改文档**（只推进运行时预览）；
/// ② 松手**恰好一条 Undo**，文档里的 Transform 真的变了；
/// ③ Undo 精确回到起点；④ 相位回到 Idle 且拖拽状态清空。
#[test]
fn synthetic_pointer_drag_commits_exactly_one_undo() {
    let mut app = interaction_app();
    app.set_scene_tool(SceneTool::Move);
    app.snap_enabled = false;
    // 合成一个普通 2D 实体（带 Sprite 框，保证命中测试有包围框）。
    let id = SceneEntityId(999);
    {
        let scene = &mut app.document.current_scene.as_mut().unwrap().scene;
        scene.add_entity(id, "DragMe", None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".to_owned(),
            "position=100,100,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
        );
        entity.components.insert(
            "Sprite".to_owned(),
            rustlua_render::sprite::SpriteComponent {
                texture: "assets/textures/player.png".to_owned(),
                size: rustlua_ecs::transform::Vec2::new(32.0, 32.0),
                pivot: rustlua_ecs::transform::Vec2::new(0.5, 0.5),
                tint: [1.0; 4],
                sorting: rustlua_render::sprite::SpriteSorting::default(),
                flip_x: false,
                flip_y: false,
                z: 0.0,
                nine_slice: None,
            }
            .to_component_str(),
        );
    }
    let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
    let entities = app.collect_scene_2d_entities(
        rect,
        &crate::panels::hierarchy::HierarchyFilter::default(),
        1.0,
    );
    let target = entities
        .iter()
        .find(|entity| entity.id == id)
        .expect("合成实体必须出现在 2D 绘制列表里");
    let start = target.center;
    let before =
        app.document.current_scene.as_ref().unwrap().scene.entities[&id].components["Transform"]
            .clone();

    // ① 按下 + 两帧拖动：只有预览，文档与历史不动。
    assert!(matches!(
        app.scene_pointer_down(start, &entities, false),
        ToolPhase::Dragging { .. }
    ));
    assert_eq!(app.selection.primary(), Some(id), "命中实体必须成为选中");
    for offset in [egui::vec2(6.0, 3.0), egui::vec2(18.0, 9.0)] {
        let pos = start + offset;
        app.scene_pointer_drag(pos);
        app.scene_pointer_drag_apply(pos, 1.0);
        assert_eq!(app.history.undo_len(), 0, "拖动中不得写历史");
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.entities[&id].components["Transform"],
            before,
            "拖动中不得改文档（预览只进运行时）"
        );
        assert!(!app.scene_state.preview.is_empty(), "拖动中必须有预览");
    }

    // ② 松手：恰好一条 Undo + 文档落到新位置。
    let end = start + egui::vec2(24.0, 12.0);
    app.scene_pointer_drag(end);
    app.scene_pointer_drag_apply(end, 1.0);
    let phase = app.scene_pointer_up(end);
    assert_eq!(phase, ToolPhase::Idle);
    assert!(app.scene_state.drag.is_none());
    assert!(app.scene_state.preview.is_empty(), "提交后预览必须清空");
    assert_eq!(app.history.undo_len(), 1, "一次拖动 = 恰好一条 Undo");
    let after =
        app.document.current_scene.as_ref().unwrap().scene.entities[&id].components["Transform"]
            .clone();
    assert_ne!(after, before, "文档必须真的被改动");
    let moved =
        rustlua_ecs::spatial_transform::LocalTransform::transform_2d_from_component_str(&after)
            .expect("统一写法必须能被 2D 视图读回");
    // 面板 Y 向下、世界 Y 向上：面板 +(24,12) 对应世界 (124, 88)（zoom=1、ppp=1）。
    assert!(
        (moved.position.x - 124.0).abs() < 0.001 && (moved.position.y - 88.0).abs() < 0.001,
        "Move 工具下位移应等于面板增量（Y 轴取反）：{:?}",
        moved.position
    );

    // ③ Undo 精确回到起点。
    assert_eq!(
        app.dispatch(editor::EditorCommand::Undo),
        editor::CommandResult::Applied
    );
    assert_eq!(
        app.document.current_scene.as_ref().unwrap().scene.entities[&id].components["Transform"],
        before,
        "Undo 必须回到拖动前的原文"
    );
}

/// **旋转吸附 + 一次撤销**（M27-02 缺项 3/7）：2D 此前完全没有角度吸附（开关只管位移），
/// 而且"轨道用的角"与"写进文档的角"必须始终是同一个数——否则吸附时画面转到位、
/// 文档里存的是另一个无理数。
#[test]
fn a_2d_rotation_drag_snaps_to_the_angle_step_and_commits_once() {
    let mut app = interaction_app();
    app.set_scene_tool(SceneTool::Rotate);
    app.snap_enabled = true;
    app.snap_angle_step = 15.0;
    let id = SceneEntityId(997);
    {
        let scene = &mut app.document.current_scene.as_mut().unwrap().scene;
        scene.add_entity(id, "SpinMe", None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".to_owned(),
            "position=100,100,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
        );
        entity.components.insert(
            "Sprite".to_owned(),
            rustlua_render::sprite::SpriteComponent {
                texture: "assets/textures/player.png".to_owned(),
                size: rustlua_ecs::transform::Vec2::new(32.0, 32.0),
                pivot: rustlua_ecs::transform::Vec2::new(0.5, 0.5),
                tint: [1.0; 4],
                sorting: rustlua_render::sprite::SpriteSorting::default(),
                flip_x: false,
                flip_y: false,
                z: 0.0,
                nine_slice: None,
            }
            .to_component_str(),
        );
    }
    let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
    let entities = app.collect_scene_2d_entities(
        rect,
        &crate::panels::hierarchy::HierarchyFilter::default(),
        1.0,
    );
    let start = entities
        .iter()
        .find(|entity| entity.id == id)
        .expect("合成实体必须出现在 2D 绘制列表里")
        .center;
    assert!(matches!(
        app.scene_pointer_down(start, &entities, false),
        ToolPhase::Dragging { .. }
    ));
    // 横向拖 100 点：原始转角 50°，15° 步长下应跳到 45°。
    let end = start + egui::vec2(100.0, 0.0);
    app.scene_pointer_drag(end);
    app.scene_pointer_drag_apply(end, 1.0);
    let previewed = app.scene_state.preview[&id];
    assert!(
        (previewed.rotation - 45.0).abs() < 1e-3,
        "预览角度必须已吸附：{}",
        previewed.rotation
    );
    assert_eq!(app.history.undo_len(), 0, "拖动中不得写历史");
    app.scene_pointer_up(end);
    assert_eq!(app.history.undo_len(), 1, "一次旋转拖拽 = 恰好一条 Undo");
    let after =
        app.document.current_scene.as_ref().unwrap().scene.entities[&id].components["Transform"]
            .clone();
    let stored = parse_transform(&after).expect("统一写法必须能被 2D 视图读回");
    assert!(
        (stored.rotation - previewed.rotation).abs() < 1e-3,
        "文档里存的必须就是画面转到的角度：{stored:?} vs {previewed:?}"
    );
    assert!(
        (stored.position_x - 100.0).abs() < 1e-2 && (stored.position_y - 100.0).abs() < 1e-2,
        "Pivot 轴心下应绕自己转，位置不动：{stored:?}"
    );
    assert_eq!(
        app.dispatch(editor::EditorCommand::Undo),
        editor::CommandResult::Applied
    );
    let before = parse_transform(
        &app.document.current_scene.as_ref().unwrap().scene.entities[&id].components["Transform"],
    )
    .expect("原文必须能读回");
    assert!(before.rotation.abs() < 1e-3, "Undo 必须回到 0°：{before:?}");
}

/// 工具切换走状态机：切换工具会**取消进行中的交互**（不再要求调用方自己清场）。
#[test]
fn tool_switch_cancels_active_interaction_state() {
    let mut app = interaction_app();
    assert_eq!(app.scene_tool(), SceneTool::Select);
    assert_eq!(app.interaction.phase(), ToolPhase::Idle);
    // 模拟 Scene View 收到一次"按在实体上"的指针事件。
    app.interaction.handle(InteractionEvent::PointerDown {
        pos: [10.0, 10.0],
        hit: true,
    });
    assert!(app.interaction.phase().is_active());
    app.set_scene_tool(SceneTool::Move);
    assert_eq!(app.scene_tool(), SceneTool::Move);
    assert_eq!(
        app.interaction.phase(),
        ToolPhase::Idle,
        "切换工具必须取消进行中的交互"
    );
}

/// Escape 取消（`clear_active_scene_drag`）必须让状态机回到 Idle：
/// 否则下一帧还会按"仍在框选/拖动"处理指针，出现取消后残留的框。
#[test]
fn escape_cancel_resets_interaction_phase() {
    let mut app = interaction_app();
    app.interaction.handle(InteractionEvent::PointerDown {
        pos: [4.0, 8.0],
        hit: false,
    });
    assert!(
        app.interaction.phase().affects_selection(),
        "空白处按下 = 框选"
    );
    app.clear_active_scene_drag();
    assert_eq!(app.interaction.phase(), ToolPhase::Idle);
    assert!(app.scene_state.drag.is_none());
}

/// 空白处按下 = 框选、实体上按下 = 拖动：这条规则由状态机判定，
/// 面板不再自己写 `box_select_start`（该字段已删除）。
#[test]
fn pointer_down_phase_follows_hit_test() {
    let mut app = interaction_app();
    app.interaction.handle(InteractionEvent::PointerDown {
        pos: [1.0, 1.0],
        hit: false,
    });
    assert!(matches!(app.interaction.phase(), ToolPhase::Marquee { .. }));
    app.interaction
        .handle(InteractionEvent::PointerUp { pos: [1.0, 1.0] });
    app.interaction.handle(InteractionEvent::PointerDown {
        pos: [1.0, 1.0],
        hit: true,
    });
    assert!(matches!(
        app.interaction.phase(),
        ToolPhase::Dragging { .. }
    ));
}
