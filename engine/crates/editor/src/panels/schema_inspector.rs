//! 通用（schema 驱动）组件字段编辑器（docs/55 阶段 2「schema 驱动 Inspector」）。
//!
//! 这一块的存在理由是计划里的硬要求：**新增组件不再修改巨型 match**。凡是引擎里注册了
//! schema、编辑器又没有手写面板的组件（`editor::driven_components` 推导出来的集合），
//! 都在这里按 schema 自动生成输入行；视图**只发 typed action**
//! （`EditorCommand::SetComponentField`），取值校验、Play 锁定、"无变化不产生历史"
//! 全部由 shell 的 dispatch 统一处理。
//!
//! 输入缓冲用 egui 的临时数据（按 `实体 + 路径` 建键）而不是新增 `EditorApp` 字段：
//! 未获得焦点时缓冲跟随文档（Undo/Redo、外部改动都会立刻反映），获得焦点时保留用户
//! 正在输入的内容。

use crate::EditorApp;
use editor::{ChoiceSource, field_choice_source};
use eframe::egui;
use engine::core::component_schema::FieldKind;

/// 一行字段的输入缓冲 + 上次同步的文档取值。
#[derive(Clone)]
struct FieldBuffer {
    /// 上次从文档同步过来的文本（用于判断文档是否变了）。
    seed: String,
    /// 当前输入框内容。
    text: String,
    /// 上一帧是否持有焦点（持有焦点时不覆盖用户输入）。
    focused: bool,
}

/// 字段该用哪种控件（**纯函数**，因此"控件选择"可以脱离 egui 单测）。
///
/// 依据是 `FieldKind` + 字段名：颜色类字段（`color` / `tint` / `fill`）用取色器，
/// 数值用 `DragValue`，向量与 `宽x高` 用多个 `DragValue` 连排——这样"把某个组件的
/// 手写面板换成 schema 生成"就**不会降低操作体验**（手写面板用的也是这些控件），
/// 这是迁移面板的前提条件。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FieldWidget {
    /// 勾选框（`Bool`）。
    Checkbox,
    /// 下拉（`Enum`）。
    Choice,
    /// 单个数值（`Int`/`Number`）；`integer` 决定是否按整数步进。
    Number { integer: bool },
    /// 多分量（`Vec2/Vec3/Vec4`）；`size` 表示 `宽x高` 写法（`x` 分隔）。
    Vector { parts: usize, size: bool },
    /// 取色器（分量数 3 或 4）。
    Color { parts: usize },
    /// 文本框（`Text`，含路径/资源引用等带规则的字段）。
    Text,
}

/// 颜色类字段名（与引擎编码器里那些 0~1 分量的字段对应）。
const COLOR_FIELD_NAMES: &[&str] = &["color", "tint", "fill", "background", "bg"];

#[must_use]
pub(crate) fn field_widget(kind: FieldKind, field: &str) -> FieldWidget {
    // 候选值来自**项目资源**的字段（字体）即使类型是文本，也用下拉：这是"手写面板的字体下拉"
    // 在通用编辑器里的等价物。
    if field_choice_source(field) != ChoiceSource::Static {
        return FieldWidget::Choice;
    }
    let is_color = COLOR_FIELD_NAMES
        .iter()
        .any(|name| field.eq_ignore_ascii_case(name));
    match kind {
        FieldKind::Bool => FieldWidget::Checkbox,
        FieldKind::Enum => FieldWidget::Choice,
        FieldKind::Int => FieldWidget::Number { integer: true },
        FieldKind::Number => FieldWidget::Number { integer: false },
        FieldKind::Size => FieldWidget::Vector {
            parts: 2,
            size: true,
        },
        FieldKind::Vec2 | FieldKind::Vec3 | FieldKind::Vec4 if is_color => {
            FieldWidget::Color {
                parts: match kind {
                    FieldKind::Vec2 => 2,
                    FieldKind::Vec3 => 3,
                    _ => 4,
                },
            }
        }
        FieldKind::Vec2 => FieldWidget::Vector {
            parts: 2,
            size: false,
        },
        FieldKind::Vec3 => FieldWidget::Vector {
            parts: 3,
            size: false,
        },
        FieldKind::Vec4 => FieldWidget::Vector {
            parts: 4,
            size: false,
        },
        FieldKind::Text => FieldWidget::Text,
    }
}

impl EditorApp {
    /// 选中实体的通用字段行；没有选中/没有接管组件时为空。
    pub(crate) fn schema_field_rows(&self) -> Vec<editor::SchemaFieldRow> {
        let Some(selected) = self.selection.primary() else {
            return Vec::new();
        };
        self.current_scene()
            .and_then(|document| document.scene.entities.get(&selected))
            .map(editor::rows_for_entity)
            .unwrap_or_default()
    }

    /// 画"schema 驱动组件"区：只有接管名单里、且实体确实拥有的组件才出现。
    pub(crate) fn draw_schema_driven_components(&mut self, ui: &mut egui::Ui) {
        let Some(selected) = self.selection.primary() else {
            return;
        };
        let rows = self.schema_field_rows();
        if rows.is_empty() {
            return;
        }
        let playing = self.is_playing();
        ui.separator();
        ui.strong("组件字段（由 schema 自动生成）");
        ui.weak("这些组件没有手写面板，字段由 Component Schema + PropertyPath 驱动；每次提交一条 Undo。");
        let mut pending: Option<(String, String, String)> = None;
        let mut current_component: Option<&'static str> = None;
        for row in &rows {
            if current_component != Some(row.component) {
                current_component = Some(row.component);
                ui.add_space(2.0);
                ui.label(format!("▸ {}", row.component));
            }
            let key = egui::Id::new(("schema_field", selected, row.path.as_str()));
            let mut buffer: FieldBuffer = ui.data_mut(|data| {
                data.get_temp_mut_or_insert_with(key, || FieldBuffer {
                    seed: row.text.clone(),
                    text: row.text.clone(),
                    focused: false,
                })
                .clone()
            });
            // 文档变了（Undo/Redo/外部改动）且用户没在编辑这一行：缓冲跟随文档。
            if !buffer.focused && buffer.seed != row.text {
                buffer.seed = row.text.clone();
                buffer.text = row.text.clone();
            }
            let widget = field_widget(row.kind, row.field);
            ui.horizontal(|ui| {
                ui.add_sized([110.0, 18.0], egui::Label::new(row.field).truncate());
                if row.is_unset() {
                    ui.weak("未设置");
                }
                match widget {
                    FieldWidget::Checkbox => {
                        let mut flag = matches!(buffer.text.trim(), "true" | "1");
                        let response =
                            ui.add_enabled(!playing, egui::Checkbox::without_text(&mut flag));
                        if response.changed() {
                            pending = Some((
                                row.component.to_owned(),
                                row.field.to_owned(),
                                if flag { "true" } else { "false" }.to_owned(),
                            ));
                        }
                        buffer.focused = response.has_focus();
                    }
                    FieldWidget::Choice => {
                        let mut choice = buffer.text.clone();
                        // 候选值可能来自**项目资源**（字体字段）：那时 schema 的静态 choices
                        // 是空的，选项由编辑器提供（"默认（不指定）" + 项目里的字体路径）。
                        let project_fonts: Vec<String> =
                            if row.choices_source == editor::ChoiceSource::ProjectFonts {
                                self.project_font_paths()
                            } else {
                                Vec::new()
                            };
                        let response = ui.add_enabled_ui(!playing, |ui| {
                            egui::ComboBox::from_id_salt(key.with("combo"))
                                .selected_text(if choice.is_empty() {
                                    if project_fonts.is_empty() {
                                        "（未设置）".to_owned()
                                    } else {
                                        "默认字体（不指定）".to_owned()
                                    }
                                } else {
                                    choice.clone()
                                })
                                .show_ui(ui, |ui| {
                                    if !project_fonts.is_empty() {
                                        ui.selectable_value(
                                            &mut choice,
                                            String::new(),
                                            "默认字体（不指定）",
                                        );
                                    }
                                    for option in row.choices {
                                        ui.selectable_value(
                                            &mut choice,
                                            (*option).to_owned(),
                                            *option,
                                        );
                                    }
                                    for path in &project_fonts {
                                        ui.selectable_value(
                                            &mut choice,
                                            path.clone(),
                                            path.as_str(),
                                        );
                                    }
                                })
                        });
                        if choice != buffer.text {
                            buffer.text = choice.clone();
                            pending =
                                Some((row.component.to_owned(), row.field.to_owned(), choice));
                        }
                        buffer.focused = response.response.has_focus();
                    }
                    FieldWidget::Number { integer } => {
                        // 数值字段用 DragValue：与手写面板同一种交互（拖动调值 + 双击输入）。
                        // **显示单位**在这里生效：如 `Camera3D.fov` 显示角度、组件串存弧度。
                        let stored: f64 = buffer.text.trim().parse().unwrap_or_default();
                        let mut value = row.unit.to_display(stored);
                        let response = ui.add_enabled(
                            !playing,
                            egui::DragValue::new(&mut value)
                                .speed(if integer { 1.0 } else { 0.1 })
                                .max_decimals(if integer { 0 } else { 4 }),
                        )
                        .on_hover_text(row.hint.as_str());
                        if response.changed() {
                            let converted = row.unit.from_display(value);
                            let text = if integer {
                                format!("{}", converted.round() as i64)
                            } else {
                                format!("{converted}")
                            };
                            buffer.text = text.clone();
                            pending = Some((row.component.to_owned(), row.field.to_owned(), text));
                            buffer.focused = true;
                        } else {
                            buffer.focused = response.has_focus();
                        }
                    }
                    FieldWidget::Vector { parts, size } => {
                        // 分量逐个数：Vec2/Vec3/Vec4 与 `宽x高`（Size）分别用 `,` / `x` 连接。
                        let separator = if size { 'x' } else { ',' };
                        let mut values: Vec<f64> = buffer
                            .text
                            .split(separator)
                            .map(|part| part.trim().parse().unwrap_or_default())
                            .collect();
                        values.resize(parts, 0.0);
                        let mut changed = false;
                        let mut response = None;
                        for (index, value) in values.iter_mut().enumerate() {
                            if index > 0 {
                                ui.weak(separator.to_string());
                            }
                            let drag = ui.add_enabled(
                                !playing,
                                egui::DragValue::new(value).speed(0.1).max_decimals(4),
                            );
                            changed |= drag.changed();
                            response = Some(drag);
                        }
                        let text = values
                            .iter()
                            .map(|value| format!("{value}"))
                            .collect::<Vec<_>>()
                            .join(&separator.to_string());
                        if changed {
                            buffer.text = text.clone();
                            pending =
                                Some((row.component.to_owned(), row.field.to_owned(), text));
                            buffer.focused = true;
                        } else {
                            buffer.focused =
                                response.is_some_and(|drag| drag.has_focus());
                        }
                    }
                    FieldWidget::Color { parts } => {
                        // 颜色/染色字段用取色器：比手写面板里的 3~4 个 DragValue 更好用。
                        let mut values: Vec<f32> = buffer
                            .text
                            .split(',')
                            .map(|part| part.trim().parse().unwrap_or_default())
                            .collect();
                        values.resize(parts, 1.0);
                        while values.len() < 4 {
                            values.push(1.0);
                        }
                        let mut color = egui::Rgba::from_rgba_unmultiplied(
                            values[0], values[1], values[2], values[3],
                        );
                        let response = ui.add_enabled_ui(!playing, |ui| {
                            egui::color_picker::color_edit_button_rgba(
                                ui,
                                &mut color,
                                egui::color_picker::Alpha::OnlyBlend,
                            )
                        });
                        let response = response.inner;
                        if response.changed() {
                            let rgba = color.to_rgba_unmultiplied();
                            // 分量数保持 schema 的语义（Vec3 = 不带 alpha）。
                            let text = if parts == 3 {
                                format!("{:.3},{:.3},{:.3}", rgba[0], rgba[1], rgba[2])
                            } else {
                                format!(
                                    "{:.3},{:.3},{:.3},{:.3}",
                                    rgba[0], rgba[1], rgba[2], rgba[3]
                                )
                            };
                            buffer.text = text.clone();
                            pending =
                                Some((row.component.to_owned(), row.field.to_owned(), text));
                        }
                        buffer.focused = response.has_focus();
                    }
                    FieldWidget::Text => {
                        // 提示文本来自 schema（类型 + 领域规则，如"string（工程内相对路径）"），
                        // 规则被拒绝时的原因也由引擎给出，面板不自己发明校验。
                        let response = ui
                            .add_enabled(
                                !playing,
                                egui::TextEdit::singleline(&mut buffer.text)
                                    .desired_width(170.0)
                                    .hint_text(row.hint.clone()),
                            )
                            .on_hover_text(row.hint.as_str());
                        buffer.focused = response.has_focus();
                        // Enter 或失焦即提交（一次提交一条 Undo，不做逐键写盘）。
                        if response.lost_focus()
                            || (response.has_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                        {
                            pending = Some((
                                row.component.to_owned(),
                                row.field.to_owned(),
                                buffer.text.clone(),
                            ));
                        }
                    }
                }
                if !row.required {
                    ui.weak("可选");
                }
            });
            ui.data_mut(|data| data.insert_temp(key, buffer));
        }
        if let Some((component, field, value)) = pending {
            // typed action：字段提交不走直接写文档，统一回到 dispatch。
            self.dispatch(editor::EditorCommand::SetComponentField {
                entity: selected,
                component,
                field,
                value,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FieldWidget, field_widget};
    use crate::EditorApp;
    use engine::core::component_schema::FieldKind;
    use editor::{CommandResult, EditorCommand, ProjectInfo, SceneDocument};
    use engine::core::{logging::LogSink, scene::SceneEntityId};

    fn app_with_quad() -> EditorApp {
        let mut app = EditorApp::new(
            ProjectInfo {
                root: std::env::temp_dir().join(format!(
                    "p2-schema-inspector-{}",
                    crate::util::test_token()
                )),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        );
        let mut document = SceneDocument::new_scene("main");
        document
            .scene
            .add_entity(SceneEntityId(1), "Root", None)
            .unwrap();
        document
            .scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Quad".to_owned(), "size=64,32;color=1,1,1,1".to_owned());
        app.document.current_scene = Some(document);
        app.project_loaded = true;
        app.selection
            .set_single(SceneEntityId(1), editor::SelectionSource::Hierarchy);
        // 真实 UI 每帧都会把 Inspector 缓冲与文档对齐；测试也要对齐，否则
        // `Undo` 的帧末收敛边界会把"从未同步过的 Inspector 缓冲"先提交成一条历史，
        // 于是单次 Undo 撤销的是那条而不是本用例的字段编辑。
        app.sync_inspector(SceneEntityId(1));
        app
    }

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
        let light = engine::core::component_schema::builtin_registry()["Light"].clone();
        let widgets: Vec<(&str, FieldWidget)> = light
            .fields
            .iter()
            .map(|field| (field.name, field_widget(field.kind, field.name)))
            .collect();
        assert_eq!(
            widgets,
            vec![
                ("direction", FieldWidget::Vector { parts: 3, size: false }),
                ("color", FieldWidget::Color { parts: 3 }),
                ("intensity", FieldWidget::Number { integer: false }),
            ],
            "Light 的三个字段在通用编辑器里必须分别得到 DragValue/取色器/DragValue"
        );
    }

    /// 通用字段区只对"接管名单里的组件"生成行（手写面板的组件不重复出现）。
    #[test]
    fn schema_rows_only_cover_missing_panels() {        let mut app = app_with_quad();
        let rows = app.schema_field_rows();
        assert!(
            rows.iter().all(|row| row.component == "Quad"),
            "只有 Quad 在接管名单里：{rows:?}"
        );
        assert!(rows.iter().any(|row| row.path == "Quad.size"));
        // 加一个手写面板覆盖的组件：不得出现在通用区。
        app.document
            .current_scene
            .as_mut()
            .unwrap()
            .scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Sprite".to_owned(), "pivot=0.5,0.5".to_owned());
        let rows = app.schema_field_rows();
        assert!(rows.iter().all(|row| row.component != "Sprite"));
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
                engine::core::light::Light::default().to_component_str(),
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
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity].components
            ["Light"]
            .clone();
        assert_eq!(
            engine::core::light::Light::from_component_str(&payload)
                .expect("引擎解析器必须认得迁移后的写入")
                .intensity,
            2.5
        );

        // 3) Undo 回到默认强度（迁移不得改变 Undo 语义）。
        assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity].components
            ["Light"]
            .clone();
        assert_eq!(
            engine::core::light::Light::from_component_str(&payload)
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
                format!("mesh={}", engine::core::mesh3d::GROUND_PLANE_SENTINEL),
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
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity].components
            ["MeshRenderer"]
            .clone();
        assert_eq!(
            engine::core::mesh3d::MeshRenderer::from_component_str(&payload)
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
                value: engine::core::mesh3d::GROUND_PLANE_SENTINEL.to_owned(),
            }),
            CommandResult::Applied
        );
    }

    // ---- 字段元数据层：显示单位与动态选项源（第 23 轮） ----

    /// 显示单位表：`Camera3D.fov` 用角度显示，其余原样。
    #[test]
    fn field_units_convert_without_touching_the_stored_contract() {
        use editor::{FieldUnit, field_unit};
        assert_eq!(field_unit("Camera3D", "fov"), FieldUnit::Degrees);
        assert_eq!(field_unit("Camera3D", "near"), FieldUnit::Raw);
        assert_eq!(field_unit("Light", "intensity"), FieldUnit::Raw);
        // 往返精度：组件串里的弧度值经"显示 → 输入"回到同一个数（60° ↔ 1.0471975 rad）。
        let stored = 60.0_f64.to_radians();
        let shown = FieldUnit::Degrees.to_display(stored);
        assert!((shown - 60.0).abs() < 1e-9, "{shown}");
        let back = FieldUnit::Degrees.from_display(shown);
        assert!((back - stored).abs() < 1e-12, "{back}");
        // 原样单位不做任何换算。
        assert_eq!(FieldUnit::Raw.to_display(0.75), 0.75);
        assert_eq!(FieldUnit::Raw.from_display(0.75), 0.75);
    }

    /// 动态选项源：字体字段的候选来自项目资源，且 hint 里带上单位说明。
    #[test]
    fn font_fields_use_project_choices_and_units_show_up_in_hints() {
        use editor::{ChoiceSource, field_choice_source};
        assert_eq!(field_choice_source("font"), ChoiceSource::ProjectFonts);
        assert_eq!(field_choice_source("text"), ChoiceSource::Static);
        let light = engine::core::component_schema::builtin_registry()["Camera3D"].clone();
        let fov = light
            .fields
            .iter()
            .find(|field| field.name == "fov")
            .expect("fov 字段");
        let row = editor::SchemaFieldRow {
            component: "Camera3D",
            field: fov.name,
            path: "Camera3D.fov".to_owned(),
            kind: fov.kind,
            required: fov.required,
            choices: fov.choices,
            text: "1.0471975".to_owned(),
            present: true,
            hint: format!(
                "{}{}",
                fov.describe(),
                editor::field_unit("Camera3D", fov.name).label()
            ),
            unit: editor::field_unit("Camera3D", fov.name),
            choices_source: editor::field_choice_source(fov.name),
        };
        assert!(
            row.hint.contains("角度"),
            "hint 必须说明显示单位：{}",
            row.hint
        );
        assert_eq!(row.unit.to_display(std::f64::consts::FRAC_PI_3).round(), 60.0);
    }

    /// **迁移证据**（第 23 轮）：`WorldText` 走通用字段路径，且**字体字段拿到的是项目字体下拉**
    /// （这正是它此前不能迁移的原因）；提交一条 Undo、引擎能连中文一起读回。
    #[test]
    fn migrated_world_text_uses_project_font_choices() {
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
                "WorldText".to_owned(),
                "text=受伤-12;font_size=18;color=1,0.2,0.2;offset=0,-24;centered=true".to_owned(),
            );

        let rows = app.schema_field_rows();
        let font = rows
            .iter()
            .find(|row| row.component == "WorldText" && row.field == "font")
            .expect("WorldText.font 行");
        assert_eq!(font.choices_source, editor::ChoiceSource::ProjectFonts);
        assert_eq!(
            field_widget(font.kind, font.field),
            FieldWidget::Choice,
            "字体字段必须是下拉（项目字体列表）"
        );
        let color = rows
            .iter()
            .find(|row| row.component == "WorldText" && row.field == "color")
            .expect("WorldText.color 行");
        assert_eq!(
            field_widget(color.kind, color.field),
            FieldWidget::Color { parts: 3 }
        );

        // 提交颜色：一条 Undo，引擎解析器能读回（含中文文本）。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentField {
                entity,
                component: "WorldText".to_owned(),
                field: "color".to_owned(),
                value: "0.25,0.5,0.75".to_owned(),
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1);
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity].components
            ["WorldText"]
            .clone();
        let parsed = engine::core::world_text::WorldText::from_component_str(&payload)
            .expect("引擎解析器");
        assert_eq!(parsed.text.text, "受伤-12");
        assert_eq!(parsed.text.color, [0.25, 0.5, 0.75]);
        assert!(parsed.centered);
    }

    /// **迁移证据**（第 22 轮）：`Image` 走通用字段路径——纹理文本框、颜色取色器、
    /// 九宫格四个 DragValue；**负边框仍被拒绝**（改由 `FieldRule::NonNegative` 对向量表达），
    /// 且全零九宫格与省略语义相同（渲染侧 `any(|v| *v > 0.0)` 过滤）。
    #[test]
    fn migrated_image_component_enforces_the_schema_rule() {
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
                "Image".to_owned(),
                "texture=assets/textures/ui/panel.png;color=1,1,1".to_owned(),
            );

        // 控件类型：颜色走取色器、九宫格走四个 DragValue（与手写面板同级或更好）。
        let rows = app.schema_field_rows();
        let color = rows
            .iter()
            .find(|row| row.component == "Image" && row.field == "color")
            .expect("Image.color 行");
        assert_eq!(
            field_widget(color.kind, color.field),
            FieldWidget::Color { parts: 3 }
        );
        let slice = rows
            .iter()
            .find(|row| row.component == "Image" && row.field == "nine_slice")
            .expect("Image.nine_slice 行");
        assert_eq!(
            field_widget(slice.kind, slice.field),
            FieldWidget::Vector {
                parts: 4,
                size: false
            }
        );

        // 提交：一条 Undo，引擎能读回四边。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentField {
                entity,
                component: "Image".to_owned(),
                field: "nine_slice".to_owned(),
                value: "8,8,16,16".to_owned(),
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1);
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity].components
            ["Image"]
            .clone();
        assert_eq!(
            engine::core::ui::UiImage::from_component_str(&payload)
                .expect("引擎解析器")
                .nine_slice,
            Some([8.0, 8.0, 16.0, 16.0])
        );

        // 负边框被拒绝（原来由编辑器 set_image 拦，现在由 schema 规则拦）。
        match app.dispatch(EditorCommand::SetComponentField {
            entity,
            component: "Image".to_owned(),
            field: "nine_slice".to_owned(),
            value: "-1,0,0,0".to_owned(),
        }) {
            CommandResult::Rejected(reason) => assert!(reason.contains("负数"), "{reason}"),
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert_eq!(app.history.undo_len(), 1, "拒绝不得产生历史");

        // 全零合法（语义等同省略，渲染侧过滤）。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentField {
                entity,
                component: "Image".to_owned(),
                field: "nine_slice".to_owned(),
                value: "0,0,0,0".to_owned(),
            }),
            CommandResult::Applied
        );
    }

    /// 一次提交 = 一条历史；重复提交同一取值不产生新历史；非法取值被拒绝且不改文档。
    #[test]
    fn field_edit_is_one_undo_entry_and_idempotent() {
        let mut app = app_with_quad();
        let before = app.document.current_scene.clone();
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentField {
                entity: SceneEntityId(1),
                component: "Quad".to_owned(),
                field: "size".to_owned(),
                value: "128,64".to_owned(),
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1);
        let payload = &app.document.current_scene.as_ref().unwrap().scene.entities
            [&SceneEntityId(1)]
        .components["Quad"];
        assert_eq!(payload, "size=128.000000,64.000000;color=1,1,1,1");

        // 幂等：同一取值（以及类型等价的写法）不再产生历史。
        for value in ["128,64", "128,64"] {
            assert_eq!(
                app.dispatch(EditorCommand::SetComponentField {
                    entity: SceneEntityId(1),
                    component: "Quad".to_owned(),
                    field: "size".to_owned(),
                    value: value.to_owned(),
                }),
                CommandResult::NoOp
            );
        }
        assert_eq!(app.history.undo_len(), 1, "无变化的提交不得污染 Undo 栈");

        // 非法取值：拒绝、无历史、文档不变。
        match app.dispatch(EditorCommand::SetComponentField {
            entity: SceneEntityId(1),
            component: "Quad".to_owned(),
            field: "size".to_owned(),
            value: "64".to_owned(),
        }) {
            CommandResult::Rejected(reason) => assert!(reason.contains("size"), "{reason}"),
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert_eq!(app.history.undo_len(), 1);

        // Undo 回到最初：比较**场景内容**（`revision` 是单调递增的缓存失效计数，
        // Undo 后仍会前进，所以整份 `SceneDocument` 不该逐字段相等）。
        assert_eq!(
            app.dispatch(EditorCommand::Undo),
            CommandResult::Applied
        );
        let restored = &app.document.current_scene.as_ref().unwrap().scene;
        assert_eq!(
            Some(restored),
            before.as_ref().map(|document| &document.scene),
            "undo_len={} status={}",
            app.history.undo_len(),
            app.status
        );
        assert_eq!(
            restored.entities[&SceneEntityId(1)].components["Quad"],
            "size=64,32;color=1,1,1,1"
        );
    }

    /// 未知字段 / 不存在的实体 / 没有该组件都是 Rejected，而不是 panic 或静默成功。
    #[test]
    fn field_edit_rejects_unknown_targets() {
        let mut app = app_with_quad();
        for (component, field) in [("Quad", "nope"), ("Sprite", "order")] {
            match app.dispatch(EditorCommand::SetComponentField {
                entity: SceneEntityId(1),
                component: component.to_owned(),
                field: field.to_owned(),
                value: "1".to_owned(),
            }) {
                CommandResult::Rejected(reason) => assert!(!reason.is_empty()),
                other => panic!("expected Rejected for {component}.{field}, got {other:?}"),
            }
        }
        match app.dispatch(EditorCommand::SetComponentField {
            entity: SceneEntityId(42),
            component: "Quad".to_owned(),
            field: "size".to_owned(),
            value: "8,8".to_owned(),
        }) {
            CommandResult::Rejected(reason) => assert!(reason.contains("42"), "{reason}"),
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert_eq!(app.history.undo_len(), 0);
    }

    /// Play 中字段编辑被拒绝（运行态与编辑态隔离）。
    #[test]
    fn field_edit_is_rejected_while_playing() {
        let mut app = app_with_quad();
        app.play
            .set_state_for_test(editor::PlayState::Playing);
        let before = app.document.current_scene.clone();
        match app.dispatch(EditorCommand::SetComponentField {
            entity: SceneEntityId(1),
            component: "Quad".to_owned(),
            field: "size".to_owned(),
            value: "8,8".to_owned(),
        }) {
            CommandResult::Rejected(_) => {}
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert_eq!(app.history.undo_len(), 0);
        assert_eq!(app.document.current_scene, before);
    }
}
