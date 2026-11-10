use super::{
    CommandResult, EditorCommand, FieldKind, FieldWidget, SceneEntityId, app_with_quad,
    field_widget,
};

// ---- 控件选择（纯函数，脱离 egui 单测） ----

/// 每种 `FieldKind` 都要映射到"与手写面板同级"的控件：
/// 数值/向量用 DragValue、颜色用取色器、枚举用下拉、布尔用勾选框、文本用输入框。
#[test]
fn field_widget_matches_kind_and_name() {
    use FieldWidget::*;
    assert_eq!(field_widget(FieldKind::Bool, "enabled"), Checkbox);
    assert_eq!(field_widget(FieldKind::Enum, "mode"), Choice);
    assert_eq!(
        field_widget(FieldKind::Int, "order"),
        Number { integer: true }
    );
    assert_eq!(
        field_widget(FieldKind::Number, "intensity"),
        Number { integer: false }
    );
    assert_eq!(
        field_widget(FieldKind::Size, "size"),
        Vector {
            parts: 2,
            size: true
        }
    );
    assert_eq!(
        field_widget(FieldKind::Vec2, "pivot"),
        Vector {
            parts: 2,
            size: false
        }
    );
    assert_eq!(
        field_widget(FieldKind::Vec3, "direction"),
        Vector {
            parts: 3,
            size: false
        }
    );
    assert_eq!(
        field_widget(FieldKind::Vec4, "bounds"),
        Vector {
            parts: 4,
            size: false
        }
    );
    assert_eq!(field_widget(FieldKind::Text, "file"), Text);
    // 字体字段的候选值来自项目资源 → 即使类型是文本也用下拉（等价于手写面板的字体下拉）。
    assert_eq!(field_widget(FieldKind::Text, "font"), Choice);
    // 颜色类字段名（任意大小写）走取色器。
    assert_eq!(field_widget(FieldKind::Vec3, "color"), Color { parts: 3 });
    assert_eq!(field_widget(FieldKind::Vec4, "Color"), Color { parts: 4 });
    assert_eq!(field_widget(FieldKind::Vec4, "tint"), Color { parts: 4 });
}

/// 关键回归点：`Sprite.tint`（颜色）、`Light.color`（颜色）、`Light.direction`（向量）
/// 这些**手写面板已经在用 DragValue/取色器**的字段，换成 schema 生成后控件类型不变——
/// 否则"迁移面板"就会变成体验降级。
#[test]
fn migrated_components_keep_their_handwritten_widget_kinds() {
    let light = rustlua_ecs::component_schema::builtin_registry()["Light"].clone();
    let widgets: Vec<(&str, FieldWidget)> = light
        .fields
        .iter()
        .map(|field| (field.name, field_widget(field.kind, field.name)))
        .collect();
    assert_eq!(
        widgets,
        vec![
            (
                "direction",
                FieldWidget::Vector {
                    parts: 3,
                    size: false
                }
            ),
            ("color", FieldWidget::Color { parts: 3 }),
            ("intensity", FieldWidget::Number { integer: false }),
            // P3-R221：点光的三个键（`type/position/range`）进 schema 之后各自的控件形态。
            // 这条断言同时是"点光可以出现在面板上"的口径：`type` 走下拉而不是文本框，
            // 引擎解析器只认 directional/point 两个字面量。
            ("type", FieldWidget::Choice),
            (
                "position",
                FieldWidget::Vector {
                    parts: 3,
                    size: false
                }
            ),
            ("range", FieldWidget::Number { integer: false }),
        ],
        "Light 的六个字段在通用编辑器里必须分别得到 DragValue/取色器/DragValue/下拉/DragValue×3/DragValue"
    );
}

/// 通用字段区只对"接管名单里的组件"生成行（手写面板的组件不重复出现）。
#[test]
fn schema_rows_only_cover_missing_panels() {
    let mut app = app_with_quad();
    let rows = app.schema_field_rows();
    assert!(
        rows.iter().all(|row| row.component == "Quad"),
        "只有 Quad 在接管名单里：{rows:?}"
    );
    assert!(rows.iter().any(|row| row.path == "Quad.size"));
    // 加一个**仍有手写面板**的组件：不得出现在通用区（`Sprite` 已在第 34 轮迁走，
    // 这里改用 `Container`——它的面板还没迁移）。
    app.document
        .current_scene
        .as_mut()
        .unwrap()
        .scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Container".to_owned(), "kind=vbox".to_owned());
    let rows = app.schema_field_rows();
    assert!(rows.iter().all(|row| row.component != "Container"));
}

/// **迁移证据**（第 20 轮）：`Light` 已经完全没有手写面板——字段行由 schema 生成、
/// 控件是 DragValue/取色器、提交走 `SetComponentField`、写出的串能被引擎解析器读回、
/// Undo 一步回滚。这条测试就是"组件迁移到 schema"之后应该长的样子。
#[test]
fn migrated_light_component_edits_through_the_typed_command() {
    let mut app = app_with_quad();
    let entity = SceneEntityId(1);
    app.document
        .current_scene
        .as_mut()
        .unwrap()
        .scene
        .entities
        .get_mut(&entity)
        .unwrap()
        .components
        .insert(
            "Light".to_owned(),
            rustlua_render::light::Light::default().to_component_str(),
        );

    // 1) 行由 schema 生成，且控件类型与手写面板同级（DragValue / 取色器）。
    let rows = app.schema_field_rows();
    let intensity = rows
        .iter()
        .find(|row| row.component == "Light" && row.field == "intensity")
        .expect("Light.intensity 行");
    assert_eq!(
        field_widget(intensity.kind, intensity.field),
        FieldWidget::Number { integer: false }
    );
    let color = rows
        .iter()
        .find(|row| row.component == "Light" && row.field == "color")
        .expect("Light.color 行");
    assert_eq!(
        field_widget(color.kind, color.field),
        FieldWidget::Color { parts: 3 }
    );

    // 2) 提交走 typed 命令：一条 Undo、文档真的改了、串仍能被引擎解析。
    assert_eq!(
        app.dispatch(EditorCommand::SetComponentField {
            entity,
            component: "Light".to_owned(),
            field: "intensity".to_owned(),
            value: "2.5".to_owned(),
        }),
        CommandResult::Applied
    );
    assert_eq!(app.history.undo_len(), 1);
    let payload =
        app.document.current_scene.as_ref().unwrap().scene.entities[&entity].components["Light"]
            .clone();
    assert_eq!(
        rustlua_render::light::Light::from_component_str(&payload)
            .expect("引擎解析器必须认得迁移后的写入")
            .intensity,
        2.5
    );

    // 3) Undo 回到默认强度（迁移不得改变 Undo 语义）。
    assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
    let payload =
        app.document.current_scene.as_ref().unwrap().scene.entities[&entity].components["Light"]
            .clone();
    assert_eq!(
        rustlua_render::light::Light::from_component_str(&payload)
            .expect("引擎解析器")
            .intensity,
        1.0
    );
}

/// **迁移证据**（第 21 轮）：`MeshRenderer` 走通用字段路径，且**语义规则比原手写面板更严**——
/// 原来只有编辑器的输入校验拦截 `.obj`，现在 schema 规则同时作用于审计与通用编辑器
/// （非法扩展名 / 穿越路径 / 绝对路径都拒绝，内置哨兵放行）。
#[test]
fn migrated_mesh_renderer_enforces_the_schema_rule() {
    let mut app = app_with_quad();
    let entity = SceneEntityId(1);
    app.document
        .current_scene
        .as_mut()
        .unwrap()
        .scene
        .entities
        .get_mut(&entity)
        .unwrap()
        .components
        .insert(
            "MeshRenderer".to_owned(),
            format!("mesh={}", rustlua_render::mesh3d::GROUND_PLANE_SENTINEL),
        );

    // 行由 schema 生成，hint 里带上规则说明（"工程内相对路径（扩展名 glb/gltf）…"）。
    let rows = app.schema_field_rows();
    let mesh = rows
        .iter()
        .find(|row| row.component == "MeshRenderer" && row.field == "mesh")
        .expect("MeshRenderer.mesh 行");
    assert!(mesh.hint.contains("glb"), "{}", mesh.hint);
    assert_eq!(field_widget(mesh.kind, mesh.field), FieldWidget::Text);

    // 合法值：一次 Undo、引擎能读回。
    assert_eq!(
        app.dispatch(EditorCommand::SetComponentField {
            entity,
            component: "MeshRenderer".to_owned(),
            field: "mesh".to_owned(),
            value: "assets/models/cube.glb".to_owned(),
        }),
        CommandResult::Applied
    );
    assert_eq!(app.history.undo_len(), 1);
    let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
        .components["MeshRenderer"]
        .clone();
    assert_eq!(
        rustlua_render::mesh3d::MeshRenderer::from_component_str(&payload)
            .expect("引擎解析器")
            .mesh,
        "assets/models/cube.glb"
    );

    // 非法值：`Rejected` 且不改文档、不加历史（原来手写面板的 `.obj` 校验没有丢）。
    for value in [
        "assets/models/cube.obj",
        "../escape.glb",
        "/etc/x.glb",
        "assets/models/cube",
    ] {
        match app.dispatch(EditorCommand::SetComponentField {
            entity,
            component: "MeshRenderer".to_owned(),
            field: "mesh".to_owned(),
            value: value.to_owned(),
        }) {
            CommandResult::Rejected(reason) => assert!(!reason.is_empty(), "{value}"),
            other => panic!("{value} 应当被拒绝，实际 {other:?}"),
        }
    }
    assert_eq!(app.history.undo_len(), 1, "拒绝不得产生历史");

    // 内置哨兵放行（"添加组件"的默认值就是它）。
    assert_eq!(
        app.dispatch(EditorCommand::SetComponentField {
            entity,
            component: "MeshRenderer".to_owned(),
            field: "mesh".to_owned(),
            value: rustlua_render::mesh3d::GROUND_PLANE_SENTINEL.to_owned(),
        }),
        CommandResult::Applied
    );
}
