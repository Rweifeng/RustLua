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
use rustlua_ecs::component_schema::FieldKind;

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
        FieldKind::Color => FieldWidget::Color { parts: 4 },
        FieldKind::Vec2 | FieldKind::Vec3 | FieldKind::Vec4 if is_color => FieldWidget::Color {
            parts: match kind {
                FieldKind::Vec2 => 2,
                FieldKind::Vec3 => 3,
                _ => 4,
            },
        },
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
    ///
    /// **多选**时以主选中为基准，其它选中实体取值不同的字段标 `mixed`（视图显示"混合"）；
    /// 提交会写到**所有选中且拥有该组件**的实体上（一条 Undo）——与旧手写面板的批量语义一致。
    pub(crate) fn schema_field_rows(&self) -> Vec<editor::SchemaFieldRow> {
        let Some(selected) = self.selection.primary() else {
            return Vec::new();
        };
        let Some(document) = self.current_scene() else {
            return Vec::new();
        };
        // 主选中在前，其余按选择顺序（批量写入的目标集合由 shell 用同一份选区算出来）。
        let mut ids = vec![selected];
        ids.extend(
            self.selection
                .ids()
                .into_iter()
                .filter(|id| *id != selected),
        );
        let entities: Vec<&rustlua_scene::scene::SceneEntity> = ids
            .iter()
            .filter_map(|id| document.scene.entities.get(id))
            .collect();
        let mut rows = editor::rows_for_selection(&entities);
        // **引用必须存在**（第 37 轮）：这是上下文校验，做不了纯字段规则——资源索引在编辑器里。
        let assets: std::collections::BTreeSet<String> = self
            .resources
            .entries()
            .iter()
            .map(editor::ResourceEntry::path_string)
            .collect();
        editor::mark_missing_assets(&mut rows, &assets);
        rows
    }

    /// 本次提交的目标数：选中集合里**确实拥有该组件**的实体数（>1 = 走批量命令）。
    fn schema_edit_targets(&self, component: &str) -> usize {
        let Some(document) = self.current_scene() else {
            return 0;
        };
        self.selection
            .ids()
            .into_iter()
            .filter(|id| {
                document
                    .scene
                    .entities
                    .get(id)
                    .is_some_and(|entity| entity.components.contains_key(component))
            })
            .count()
    }

    /// 某个组件的**只读说明**（如相机的投影摘要与生效姿态）：视图只呈现，不编辑。
    pub(crate) fn schema_component_notes(
        &self,
        component: &str,
    ) -> Vec<editor::SchemaComponentNote> {
        let Some(selected) = self.selection.primary() else {
            return Vec::new();
        };
        self.current_scene()
            .and_then(|document| document.scene.entities.get(&selected))
            .map(|entity| editor::component_notes(entity, component))
            .unwrap_or_default()
    }

    /// 某个组件的**动作**（如 `UiLayout` 的锚点预设）：视图渲染成下拉，点击发一条
    /// `EditorCommand::SetComponentFields`（原子多字段 = 一条 Undo）。
    pub(crate) fn schema_component_actions(
        &self,
        component: &str,
    ) -> Vec<editor::SchemaComponentAction> {
        let Some(selected) = self.selection.primary() else {
            return Vec::new();
        };
        self.current_scene()
            .and_then(|document| document.scene.entities.get(&selected))
            .map(|entity| editor::component_actions(entity, component))
            .unwrap_or_default()
    }

    /// 画"schema 驱动组件"区：只有接管名单里、且实体确实拥有的组件才出现。
    pub(crate) fn draw_schema_driven_components(&mut self, ui: &mut egui::Ui) {
        let Some(selected) = self.selection.primary() else {
            return;
        };
        let rows =
            editor::filter_field_rows(&self.schema_field_rows(), &self.inspector_field_filter);
        if rows.is_empty() {
            return;
        }
        let playing = self.is_playing();
        ui.separator();
        // P1：字段搜索（多词 AND，匹配组件名/字段名/路径/当前值）。过滤词只影响本面板的
        // 呈现，不改文档；清空即回到全部字段。
        // 用 right-to-left 布局：清除按钮先画（靠右），TextEdit 后画并用 INFINITY 占满剩余宽度。
        // 之前按 available_width()-34 算固定 desired_width，但 egui 的 desired_width 只是"期望"，
        // 输入文字比这个宽度长时 TextEdit 的内容 galley 会反过来撑大自身，导致输入框越长越宽。
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            // 空时也在按钮位分配 0 宽占位：保持 TextEdit 的 widget 序号稳定（同 hierarchy 搜索框）。
            if !self.inspector_field_filter.is_empty()
                && crate::util::draw_icon_button(ui, crate::util::DrawIcon::Cross, "清空搜索")
                    .clicked()
            {
                self.inspector_field_filter.clear();
            } else {
                ui.allocate_exact_size(egui::vec2(0.0, 0.0), egui::Sense::hover());
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.inspector_field_filter)
                    .id(ui.make_persistent_id("inspector_field_search_box"))
                    .hint_text("搜索字段（如 texture / camera fov）")
                    .desired_width(f32::INFINITY),
            );
        });
        if !self.inspector_field_filter.trim().is_empty() {
            let total = self.schema_field_rows().len();
            if rows.len() < total {
                ui.small(format!("匹配 {} / {} 行字段", rows.len(), total));
            } else {
                ui.small(format!("匹配 {} 行字段", rows.len()));
            }
        }
        let mut pending: Option<(String, String, String)> = None;
        // 组件级动作（原子多字段）与单字段提交分开排队：动作必须**一条**历史。
        let mut pending_action: Option<(String, Vec<(String, String)>)> = None;
        // 打开的"编辑器态开关"（如 Transform 的等比锁）：提交时会把镜像字段一起写。
        let mut active_toggles: Vec<(&'static str, Vec<editor::SchemaComponentToggle>)> =
            Vec::new();
        let mut current_component: Option<&'static str> = None;
        for (index, row) in rows.iter().enumerate() {
            if current_component != Some(row.component) {
                current_component = Some(row.component);
                ui.add_space(2.0);
                ui.label(format!("▸ {}", row.component));
                // 编辑器态开关（不写文档）：如 Transform/Transform3D 的"锁定比例"。
                let mut open_toggles = Vec::new();
                for toggle in editor::component_toggles(row.component) {
                    let key = egui::Id::new(("schema_toggle", selected, toggle.key));
                    let mut on = ui
                        .data_mut(|data| *data.get_temp_mut_or_insert_with(key, || toggle.default));
                    ui.add_enabled(!playing, egui::Checkbox::new(&mut on, toggle.label));
                    ui.data_mut(|data| data.insert_temp(key, on));
                    if on {
                        open_toggles.push(toggle.clone());
                    }
                }
                if !open_toggles.is_empty() {
                    active_toggles.push((row.component, open_toggles));
                }
                // 组件级动作（如 UiLayout 的锚点预设）：下拉 + 选中即提交。
                let actions = self.schema_component_actions(row.component);
                if !actions.is_empty() {
                    let selected_label = actions
                        .iter()
                        .find(|action| action.selected)
                        .map_or_else(|| "自定义".to_owned(), |action| action.label.clone());
                    let mut choice = selected_label;
                    ui.horizontal(|ui| {
                        ui.add_sized([84.0, 18.0], egui::Label::new("预设").truncate());
                        ui.add_enabled_ui(!playing, |ui| {
                            egui::ComboBox::from_id_salt(egui::Id::new((
                                "schema_component_action",
                                selected,
                                row.component,
                            )))
                            .selected_text(choice.clone())
                            .show_ui(ui, |ui| {
                                for action in &actions {
                                    ui.selectable_value(
                                        &mut choice,
                                        action.label.clone(),
                                        action.label.as_str(),
                                    );
                                }
                            });
                        });
                    });
                    if let Some(action) = actions.iter().find(|action| action.label == choice)
                        && !action.selected
                    {
                        pending_action = Some((row.component.to_owned(), action.fields.clone()));
                    }
                }
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
            let row_inner = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add_sized([84.0, 18.0], egui::Label::new(row.field).truncate());
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
                        let response = ui
                            .add_enabled(
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
                            pending = Some((row.component.to_owned(), row.field.to_owned(), text));
                            buffer.focused = true;
                        } else {
                            buffer.focused = response.is_some_and(|drag| drag.has_focus());
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
                            pending = Some((row.component.to_owned(), row.field.to_owned(), text));
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
                                    .desired_width(130.0)
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
                // P1：**引用选择器**（第 42 轮）。带路径规则的字段（`Sprite.texture`、
                // `MeshRenderer.mesh`、`Animation3D.mesh` …）不再只能手打路径：下拉列出
                // 工程里扩展名匹配的资源，选中即提交（一条 Undo）。
                // 候选规则住在 `editor::picker_candidates`（纯函数，与扩展名白名单一起被单测守住）。
                if row.path_rule {
                    let candidates = self.picker_candidates_for(row.path_extensions, &buffer.text);
                    if !candidates.is_empty() {
                        let current = buffer.text.trim().replace('\\', "/");
                        let mut chosen = current.clone();
                        let mut picked = false;
                        ui.add_enabled_ui(!playing, |ui| {
                            egui::ComboBox::from_id_salt(key.with("asset_picker"))
                                .selected_text("资源…")
                                .width(70.0)
                                .show_ui(ui, |ui| {
                                    if !row.required
                                        && ui
                                            .selectable_label(chosen.is_empty(), "（不指定）")
                                            .clicked()
                                    {
                                        chosen.clear();
                                        picked = true;
                                    }
                                    for path in &candidates {
                                        if ui.selectable_label(*path == chosen, path).clicked() {
                                            chosen = path.clone();
                                            picked = true;
                                        }
                                    }
                                });
                        });
                        if picked {
                            buffer.text = chosen.clone();
                            // 选择是一次明确动作：直接排队提交，不等失焦
                            // （失焦那一帧也可能排队，但随后被这次覆盖）。
                            buffer.focused = false;
                            pending =
                                Some((row.component.to_owned(), row.field.to_owned(), chosen));
                        }
                    }
                    // 「定位」：在资源面板里选中并滚到这个引用指向的条目。
                    // `missing_asset`（含 `__white__` 这类内置哨兵）在索引里没有对应行，
                    // 所以不画按钮——那一行右侧的 ⚠ 已经把原因说清了。
                    if !row.missing_asset
                        && !buffer.text.trim().is_empty()
                        && ui
                            .small_button("定位")
                            .on_hover_text("在资源面板里选中该资源并滚动到它所在的位置")
                            .clicked()
                    {
                        self.request_resource_locate(&buffer.text);
                    }
                }
                if !row.required {
                    ui.weak(egui::RichText::new("可选").small());
                }
                // 多选混合值：显示提示，但输入框仍以主选中的值为基准（提交写全部目标）。
                if row.mixed {
                    ui.weak(egui::RichText::new("混合").small());
                }
                // 引用必须存在：资源索引里找不到时给醒目提示（仍允许写入）。
                if row.missing_asset {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 150, 60),
                        "⚠ 工程内找不到该资源",
                    );
                }
            });
            ui.data_mut(|data| data.insert_temp(key, buffer));
            // M27-01：资源引用字段（`Sprite.texture`、`MeshRenderer.mesh`…）是第四块放置区。
            // 落在这行的资产直接成为字段值——与下拉选择器同一条提交路径（复用下面的 `pending`）。
            if row.path_rule
                && let Some(value) =
                    self.inspector_field_drop(ui, row_inner.response.rect, key, playing)
            {
                pending = Some((row.component.to_owned(), row.field.to_owned(), value));
            }
            // 该组件的最后一行之后画它的**只读说明**（如相机的投影摘要与生效姿态）：
            // 这些信息不是字段（没有输入控件），但它是"渲染器真正会用到的值"，
            // 迁移前住在手写面板里，迁移后由 `schema_fields::component_notes` 提供。
            let last_of_component = rows
                .get(index + 1)
                .is_none_or(|next| next.component != row.component);
            if last_of_component {
                for note in self.schema_component_notes(row.component) {
                    if note.warning {
                        ui.colored_label(egui::Color32::from_rgb(220, 150, 60), &note.text);
                    } else {
                        ui.weak(&note.text);
                    }
                }
            }
        }
        if let Some((component, fields)) = pending_action {
            // 组件级动作：原子多字段，一条 Undo（与单字段提交互斥：同一帧只会有一个排队）。
            // 多选且多个目标都有该组件时走**批量**命令（仍然只有一条 Undo）。
            if self.schema_edit_targets(&component) > 1 {
                self.dispatch(editor::EditorCommand::SetComponentFieldsOnSelection {
                    component,
                    fields,
                });
            } else {
                self.dispatch(editor::EditorCommand::SetComponentFields {
                    entity: selected,
                    component,
                    fields,
                });
            }
        } else if let Some((component, field, value)) = pending {
            // 字段镜像（等比锁）：把镜像字段一起写，仍然只有一条 Undo。
            let mirrored: Vec<(String, String)> = active_toggles
                .iter()
                .filter(|(name, _)| *name == component)
                .flat_map(|(_, toggles)| editor::mirror_fields(toggles, &field, &value))
                .collect();
            if mirrored.is_empty() {
                // typed action：字段提交不走直接写文档，统一回到 dispatch（多选时批量写）。
                if self.schema_edit_targets(&component) > 1 {
                    self.dispatch(editor::EditorCommand::SetComponentFieldOnSelection {
                        component,
                        field,
                        value,
                    });
                } else {
                    self.dispatch(editor::EditorCommand::SetComponentField {
                        entity: selected,
                        component,
                        field,
                        value,
                    });
                }
            } else {
                let mut fields = vec![(field, value)];
                fields.extend(mirrored);
                if self.schema_edit_targets(&component) > 1 {
                    self.dispatch(editor::EditorCommand::SetComponentFieldsOnSelection {
                        component,
                        fields,
                    });
                } else {
                    self.dispatch(editor::EditorCommand::SetComponentFields {
                        entity: selected,
                        component,
                        fields,
                    });
                }
            }
        }
    }

    /// M27-01：一行资源引用字段的放置区——返回"这一行该写入的新值"（`None` = 本帧无落点）。
    ///
    /// 为什么不用 `dnd_drop_zone`：它会把内容包进一个 `Frame` 并改变行布局，而 Inspector 的
    /// 行高受"内容不得高于行高"的门槛断言管着。这里用它释放判定的**等价物**——hover 响应 +
    /// `dnd_release_payload`（egui 里就是"指针在内 && 松手 ⇒ `take_payload`"）。
    ///
    /// 只在确实有拖拽进行时才登记这块 hover 区：一个常年盖在输入框上的 hover 区会抢走
    /// 下面控件的 tooltip。
    fn inspector_field_drop(
        &mut self,
        ui: &mut egui::Ui,
        row_rect: egui::Rect,
        key: egui::Id,
        playing: bool,
    ) -> Option<String> {
        if playing {
            // 仍然**认领**这一帧的系统拖放：不认领的话帧末扫尾会说"落点不在可放置的面板内"，
            // 而真实原因是 Play 锁定。
            if let Some((_, drop)) =
                self.claim_external_drop(crate::dnd::DropZone::InspectorField, row_rect)
            {
                self.status = format!(
                    "Play mode locks scene editing; click Stop first{}",
                    drop.notes()
                );
            }
            return None;
        }
        let internal = crate::dnd::internal_drag(ui.ctx());
        if internal.is_none() && self.external_hover.is_empty() {
            return None;
        }
        let zone = ui.interact(row_rect, key.with("asset_drop"), egui::Sense::hover());
        if let Some(payload) = zone.dnd_release_payload::<crate::dnd::DropPayload>() {
            let source = crate::dnd::DragSource::Internal((*payload).clone());
            let action = crate::dnd::plan(&source, crate::dnd::DropZone::InspectorField);
            return self.commit_field_ref_drop(&action, String::new());
        }
        if let Some((action, drop)) =
            self.claim_external_drop(crate::dnd::DropZone::InspectorField, row_rect)
        {
            let notes = drop.notes();
            return self.commit_field_ref_drop(&action, notes);
        }
        // 还没松手：把这行"松手会得到什么"画出来（合法绿框 / 非法红框都出自同一张矩阵）。
        let pointer = ui.input(|input| input.pointer.interact_pos());
        let hovering = self
            .external_hover_action(crate::dnd::DropZone::InspectorField, row_rect, pointer)
            .or_else(|| {
                internal
                    .filter(|_| pointer.is_some_and(|position| row_rect.contains(position)))
                    .map(|source| crate::dnd::plan(&source, crate::dnd::DropZone::InspectorField))
            });
        if let Some(action) = hovering {
            crate::dnd::paint_drop_feedback(ui, row_rect, 5, &action);
        }
        None
    }

    /// 落点动作 → 字段新值。工程外的单文件先入库（复制失败就不写引用），拒绝只说话。
    fn commit_field_ref_drop(
        &mut self,
        action: &crate::dnd::DropAction,
        notes: String,
    ) -> Option<String> {
        use crate::dnd::DropAction;
        match action {
            DropAction::SetAssetRef(path) => Some(path.clone()),
            DropAction::ImportAndSetRef(file) => {
                let report = self.import_external_files(std::slice::from_ref(file));
                let relative = report.imported.first().cloned();
                self.status = if relative.is_some() {
                    format!("{}{notes}", report.status_text())
                } else {
                    report.status_text()
                };
                relative
            }
            DropAction::Reject(reason) => {
                self.status = format!("{reason}{notes}");
                None
            }
            other => {
                self.status = format!("资源引用字段收不下这个拖放：{}", other.caption());
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FieldWidget, field_widget};
    use crate::EditorApp;
    use editor::{CommandResult, EditorCommand, ProjectInfo, SceneDocument};
    use rustlua_ecs::component_schema::FieldKind;
    use rustlua_infra::logging::LogSink;
    use rustlua_scene::scene::SceneEntityId;

    fn app_with_quad() -> EditorApp {
        let mut app = EditorApp::new(
            ProjectInfo {
                root: std::env::temp_dir().join(format!(
                    "rustlua-schema-inspector-{}",
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
            ],
            "Light 的三个字段在通用编辑器里必须分别得到 DragValue/取色器/DragValue"
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
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["Light"]
            .clone();
        assert_eq!(
            rustlua_render::light::Light::from_component_str(&payload)
                .expect("引擎解析器必须认得迁移后的写入")
                .intensity,
            2.5
        );

        // 3) Undo 回到默认强度（迁移不得改变 Undo 语义）。
        assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["Light"]
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

    // ---- 字段元数据层：显示单位与动态选项源（第 23 轮） ----

    /// 显示单位表：`Camera3D.fov` 用角度（**组件串里存的就是角度**，所以编辑器不做换算），
    /// 其余原样。旧写法把 `Degrees` 当成"组件串存弧度"，那会让 60° 显示成 3437.75°——
    /// 迁移 `Camera3D` 时正是靠这条测试与 `field_units_agree_with_the_engine_encoder`
    /// 的引擎对拍把这个漂移钉住。
    #[test]
    fn field_units_convert_without_touching_the_stored_contract() {
        use editor::{FieldUnit, field_unit};
        assert_eq!(field_unit("Camera", "fov"), FieldUnit::Degrees);
        assert_eq!(field_unit("Camera", "near"), FieldUnit::Raw);
        assert_eq!(field_unit("Light", "intensity"), FieldUnit::Raw);
        // `Degrees` = 组件串里就是角度：显示与存储是同一个数（不换算）。
        assert_eq!(FieldUnit::Degrees.to_display(60.0), 60.0);
        assert_eq!(FieldUnit::Degrees.from_display(60.0), 60.0);
        // 原样单位不做任何换算。
        assert_eq!(FieldUnit::Raw.to_display(0.75), 0.75);
        assert_eq!(FieldUnit::Raw.from_display(0.75), 0.75);
        // `Radians` 才是"组件串存弧度、编辑器显示角度"，往返精度由它保证。
        let stored = 60.0_f64.to_radians();
        assert!((FieldUnit::Radians.to_display(stored) - 60.0).abs() < 1e-9);
        assert!((FieldUnit::Radians.from_display(60.0) - stored).abs() < 1e-12);
    }

    /// 动态选项源：字体字段的候选来自项目资源，且 hint 里带上单位说明。
    #[test]
    fn font_fields_use_project_choices_and_units_show_up_in_hints() {
        use editor::{ChoiceSource, field_choice_source};
        assert_eq!(field_choice_source("font"), ChoiceSource::ProjectFonts);
        assert_eq!(field_choice_source("text"), ChoiceSource::Static);
        let light = rustlua_ecs::component_schema::builtin_registry()["Camera"].clone();
        let fov = light
            .fields
            .iter()
            .find(|field| field.name == "fov")
            .expect("fov 字段");
        let row = editor::SchemaFieldRow {
            component: "Camera",
            field: fov.name,
            path: "Camera3D.fov".to_owned(),
            kind: fov.kind,
            required: fov.required,
            choices: fov.choices,
            // 组件串里存的就是角度（见 `field_units_agree_with_the_engine_encoder`）。
            text: "60".to_owned(),
            present: true,
            hint: format!(
                "{}{}",
                fov.describe(),
                editor::field_unit("Camera", fov.name).label()
            ),
            unit: editor::field_unit("Camera", fov.name),
            choices_source: editor::field_choice_source(fov.name),
            mixed: false,
            path_rule: false,
            missing_asset: false,
            path_extensions: &[],
        };
        assert!(
            row.hint.contains("角度"),
            "hint 必须说明显示单位：{}",
            row.hint
        );
        // `Degrees` = 组件串里就是角度：显示值就是原文（换算只发生在 `Radians` 字段上）。
        assert_eq!(row.unit.to_display(60.0), 60.0);
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
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["WorldText"]
            .clone();
        let parsed =
            rustlua_ui::world_text::WorldText::from_component_str(&payload).expect("引擎解析器");
        assert_eq!(parsed.text.text, "受伤-12");
        assert_eq!(parsed.text.color, [0.25, 0.5, 0.75]);
        assert!(parsed.centered);
    }

    /// **迁移证据**（第 22 轮）：`Image` 走通用字段路径——纹理文本框、颜色取色器、
    /// 九宫格四个 DragValue；**负边框仍被拒绝**（改由 `FieldRule::NonNegative` 对向量表达），
    /// **Inspector 字段搜索 + 引用选择器**（第 42 轮 P1，视图侧证据）：
    /// ① 字段行带着**扩展名白名单**（选择器的候选规则与 `FieldRule::ProjectPath` 同源）；
    /// ② 搜索过滤后视图只画匹配的行；③ 选择器候选来自工程资源索引（这里用真实索引喂）。
    #[test]
    fn field_search_and_reference_picker_are_driven_by_the_schema() {
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
                "Sprite".to_owned(),
                "texture=assets/textures/player.png;size=1x1;tint=1,1,1,1;sorting_layer=default"
                    .to_owned(),
            );

        let rows = app.schema_field_rows();
        let texture = rows
            .iter()
            .find(|row| row.path == "Sprite.texture")
            .expect("Sprite.texture 行");
        assert!(texture.path_rule, "纹理字段必须是路径规则字段");
        assert_eq!(
            texture.path_extensions.to_vec(),
            vec!["png", "jpg", "jpeg", "webp"],
            "扩展名白名单必须与引擎的 FieldRule 同一份事实源"
        );
        // 颜色字段不是路径规则（不会长出选择器）。
        let tint = rows
            .iter()
            .find(|row| row.path == "Sprite.tint")
            .expect("Sprite.tint 行");
        assert!(!tint.path_rule);
        assert!(tint.path_extensions.is_empty());

        // ② 搜索：`sprite texture` 只留下这一行；`nope` 什么都不留。
        assert_eq!(
            editor::filter_field_rows(&rows, "sprite texture")
                .iter()
                .map(|row| row.path.as_str())
                .collect::<Vec<_>>(),
            vec!["Sprite.texture"]
        );
        assert!(editor::filter_field_rows(&rows, "nope").is_empty());
        // 视图状态确实被搜索框驱动（面板读的就是这个字段）。
        app.inspector_field_filter = "sprite texture".to_owned();
        assert_eq!(
            editor::filter_field_rows(&app.schema_field_rows(), &app.inspector_field_filter).len(),
            1
        );

        // ③ 选择器候选：按扩展名筛 + 当前值兜底（换行写成工程里更常见的反斜杠形式）。
        let candidates =
            app.picker_candidates_for(texture.path_extensions, "assets\\textures\\player.png");
        assert_eq!(candidates[0], "assets/textures/player.png");
        let empty = app.picker_candidates_for(&[], "");
        assert!(
            empty.is_empty()
                || empty
                    .iter()
                    .all(|path| path.ends_with(".png") || path.contains('.')),
            "只应给出真实资源路径：{empty:?}"
        );
    }

    /// **第 39 轮新接管的组件**（2D `Collider` / `Animation3D`）：在这之前它们在编辑器里
    /// **没有任何字段行**（两者都没有 schema）。注册 schema 之后通用编辑器**自动**接管，
    /// 这里按迁移样板验证"控件同级 + typed 命令 + 一条 Undo + 引擎解析器读回 +
    /// 跨字段/路径不变量由引擎与 schema 拦下"。
    #[test]
    fn newly_registered_components_are_editable_through_the_generic_editor() {
        let mut app = app_with_quad();
        let entity = SceneEntityId(1);
        {
            let components = &mut app
                .document
                .current_scene
                .as_mut()
                .unwrap()
                .scene
                .entities
                .get_mut(&entity)
                .unwrap()
                .components;
            // 两份都是真实工程里的形态（`game_jyht/scenes/*.scene`、`game_api/scenes/api_3d.scene`）。
            components.insert("Collider".to_owned(), "shape=ball;radius=12".to_owned());
            components.insert(
                "Animation3D".to_owned(),
                "mesh=assets/models/kaykit-warrior.glb;clip=Idle;looped=true".to_owned(),
            );
        }

        // 1) 控件：`shape` 与 `looped` 都是候选下拉（候选集 = 引擎解析器认的字面量）。
        let rows = app.schema_field_rows();
        let shape = rows
            .iter()
            .find(|row| row.component == "Collider" && row.field == "shape")
            .expect("Collider.shape 行");
        assert_eq!(field_widget(shape.kind, shape.field), FieldWidget::Choice);
        assert_eq!(shape.choices.to_vec(), vec!["ball", "cuboid"]);
        let looped = rows
            .iter()
            .find(|row| row.component == "Animation3D" && row.field == "looped")
            .expect("Animation3D.looped 行");
        assert_eq!(field_widget(looped.kind, looped.field), FieldWidget::Choice);

        // 2) 改 2D 碰撞体半径：一条 Undo，引擎解析器读回 20。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentField {
                entity,
                component: "Collider".to_owned(),
                field: "radius".to_owned(),
                value: "20".to_owned(),
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1);
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["Collider"]
            .clone();
        let collider = rustlua_physics::physics::ColliderDesc::from_component_str(&payload)
            .expect("引擎解析器必须认得通用编辑器写出来的串");
        assert!(
            format!("{collider:?}").contains("Ball(20"),
            "半径必须落到引擎读到的形状里：{collider:?}"
        );

        // 3) 跨字段不变量（`shape=ball` 的半径必须为正）由**引擎兜底门**拦下，
        //    schema 的字段规则表达不了它——与旧面板同级，迁移不放松校验。
        match app.dispatch(EditorCommand::SetComponentField {
            entity,
            component: "Collider".to_owned(),
            field: "radius".to_owned(),
            value: "0".to_owned(),
        }) {
            CommandResult::Rejected(reason) => assert!(!reason.is_empty(), "{reason}"),
            other => panic!("ball 半径必须为正，实际 {other:?}"),
        }
        assert_eq!(app.history.undo_len(), 1, "拒绝不得产生历史");

        // 4) `Animation3D`：`clip` 是自由文本；`mesh` 挂着"工程内相对路径 + glb/gltf"规则，
        //    绝对路径必须在写入前被 schema 拦下。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentField {
                entity,
                component: "Animation3D".to_owned(),
                field: "clip".to_owned(),
                value: "Running_A".to_owned(),
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 2);
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["Animation3D"]
            .clone();
        assert!(payload.contains("clip=Running_A"), "{payload}");
        match app.dispatch(EditorCommand::SetComponentField {
            entity,
            component: "Animation3D".to_owned(),
            field: "mesh".to_owned(),
            value: "C:/models/hero.glb".to_owned(),
        }) {
            CommandResult::Rejected(reason) => assert!(!reason.is_empty(), "{reason}"),
            other => panic!("绝对路径必须被拒绝，实际 {other:?}"),
        }
        assert_eq!(app.history.undo_len(), 2, "拒绝不得产生历史");
    }

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

        // 控件类型：颜色走取色器（RGBA 四段，见 M64a）、九宫格走四个 DragValue（与手写面板同级或更好）。
        let rows = app.schema_field_rows();
        let color = rows
            .iter()
            .find(|row| row.component == "Image" && row.field == "color")
            .expect("Image.color 行");
        assert_eq!(
            field_widget(color.kind, color.field),
            FieldWidget::Color { parts: 4 }
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
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["Image"]
            .clone();
        assert_eq!(
            rustlua_ui::ui::UiImage::from_component_str(&payload)
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

    /// **迁移证据**（第 24 轮）：`Camera3D` 走通用字段路径——position/target = DragValue×3、
    /// **`fov` 以角度显示**（组件串里存的就是角度，编辑器不做任何换算）、`pose` = 下拉
    /// （含 `look_at`，否则 `pose=transform` 无法撤销）；跨字段不变量（`near < far`）
    /// 由**引擎解析器**兜底拦下；面板里"不是字段"的只读摘要由 `component_notes` 提供。
    #[test]
    fn migrated_camera3d_keeps_units_notes_and_engine_invariants() {
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
                "Camera".to_owned(),
                "projection=perspective;position=0,0,5;target=0,0,0;fov=60;near=0.1;far=100"
                    .to_owned(),
            );

        // 1) 行由 schema 生成；`fov` 显示的角度就是组件串里的数（60°，不是 3437.75°）。
        let rows = app.schema_field_rows();
        let fov = rows
            .iter()
            .find(|row| row.component == "Camera" && row.field == "fov")
            .expect("Camera3D.fov 行");
        assert_eq!(fov.unit, editor::FieldUnit::Degrees);
        assert_eq!(fov.text, "60", "原始文本原样读出（不回写、不换算）");
        assert_eq!(fov.unit.to_display(60.0), 60.0, "Degrees 不做换算");
        assert_eq!(
            field_widget(fov.kind, fov.field),
            FieldWidget::Number { integer: false }
        );
        let position = rows
            .iter()
            .find(|row| row.component == "Camera" && row.field == "position")
            .expect("Camera3D.position 行");
        assert_eq!(
            field_widget(position.kind, position.field),
            FieldWidget::Vector {
                parts: 3,
                size: false
            }
        );
        let pose = rows
            .iter()
            .find(|row| row.component == "Camera" && row.field == "pose")
            .expect("Camera3D.pose 行");
        assert_eq!(field_widget(pose.kind, pose.field), FieldWidget::Choice);
        assert!(
            pose.choices.contains(&"look_at"),
            "pose 必须有 look_at（与省略同义），否则 pose=transform 改不回去：{:?}",
            pose.choices
        );

        // 2) 提交 fov=75（角度）：一条 Undo，引擎读回 75°。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentField {
                entity,
                component: "Camera".to_owned(),
                field: "fov".to_owned(),
                value: "75".to_owned(),
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1);
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["Camera"]
            .clone();
        let camera = rustlua_camera::camera::Camera::from_component_str(&payload)
            .expect("引擎解析器必须认得迁移后的写入")
            .as_perspective()
            .expect("判别字段是 perspective");
        assert!(
            (camera.fov_y.to_degrees() - 75.0).abs() < 0.001,
            "组件串存角度，所以引擎读回来就是 75°：{}",
            camera.fov_y.to_degrees()
        );

        // 3) 跨字段不变量：`near >= far` 被**引擎解析器**拦下（schema 的字段规则表达不了），
        //    与旧手写面板的 `validate_camera3d` 同级——迁移不放松校验。
        match app.dispatch(EditorCommand::SetComponentField {
            entity,
            component: "Camera".to_owned(),
            field: "near".to_owned(),
            value: "200".to_owned(),
        }) {
            CommandResult::Rejected(reason) => assert!(!reason.is_empty(), "{reason}"),
            other => panic!("near >= far 必须被拒绝，实际 {other:?}"),
        }
        assert_eq!(app.history.undo_len(), 1, "拒绝不得产生历史");

        // 4) 只读说明：投影摘要来自统一 `Projection`；禁用时给出"不参与选帧"的警告。
        let notes = app.schema_component_notes("Camera");
        assert!(
            notes
                .iter()
                .any(|note| note.text.contains("透视") && note.text.contains("75.0°")),
            "{notes:?}"
        );
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentField {
                entity,
                component: "Camera".to_owned(),
                field: "enabled".to_owned(),
                value: "false".to_owned(),
            }),
            CommandResult::Applied
        );
        let notes = app.schema_component_notes("Camera");
        assert!(
            notes
                .iter()
                .any(|note| note.warning && note.text.contains("不参与相机选帧")),
            "{notes:?}"
        );
    }

    /// **迁移证据**（第 25 轮）：`Text` 走通用字段路径——文本 = 输入框、字号 = DragValue、
    /// **字体 = 项目字体下拉**（`ChoiceSource::ProjectFonts`）、颜色 = 取色器；
    /// 含 `;`/`=` 的自由文本由 schema 的转义字段负责，写出的串引擎仍能原样读回。
    #[test]
    fn migrated_text_keeps_font_choices_and_escaped_text() {
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
                "Text".to_owned(),
                "text=你好;font_size=16;color=1,1,1".to_owned(),
            );

        let rows = app.schema_field_rows();
        let text = rows
            .iter()
            .find(|row| row.component == "Text" && row.field == "text")
            .expect("Text.text 行");
        assert_eq!(field_widget(text.kind, text.field), FieldWidget::Text);
        let font = rows
            .iter()
            .find(|row| row.component == "Text" && row.field == "font")
            .expect("Text.font 行");
        assert_eq!(font.choices_source, editor::ChoiceSource::ProjectFonts);
        assert_eq!(
            field_widget(font.kind, font.field),
            FieldWidget::Choice,
            "字体字段必须是下拉（项目字体列表），与旧手写面板同级"
        );
        let color = rows
            .iter()
            .find(|row| row.component == "Text" && row.field == "color")
            .expect("Text.color 行");
        assert_eq!(
            field_widget(color.kind, color.field),
            FieldWidget::Color { parts: 3 },
            "颜色从 3 个 DragValue 升级为取色器"
        );

        // 提交含 `;`/`=` 的文本：一条 Undo，组件串里是转义形式，引擎读回真实文字。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentField {
                entity,
                component: "Text".to_owned(),
                field: "text".to_owned(),
                value: "a;b=中文".to_owned(),
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1);
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["Text"]
            .clone();
        let parsed = rustlua_ui::ui::UiText::from_component_str(&payload).expect("引擎解析器");
        assert_eq!(parsed.text, "a;b=中文");
        assert_eq!(parsed.font_size, 16.0, "写一个字段不得改动其它字段");
    }

    /// **迁移证据**（第 31 轮）：`UiLayout` 走通用字段路径——`anchors`/`offsets` = 4 个
    /// DragValue、`min_size`/`max_size`/`pivot` = 2 个 DragValue；**锚点预设**由组件级动作下拉
    /// 承担（12 个预设来自引擎，一次改 `anchors`+`pivot` = 一条 Undo）。
    /// anchors/pivot 的 `0..=1` 约束由**引擎解析器兜底门**保证（原手写面板的同一条规则）。
    #[test]
    fn migrated_ui_layout_keeps_fields_and_preset_actions() {
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
                "UiLayout".to_owned(),
                "anchors=0,0,0,0;offsets=0,0,0,0;min_size=0,0;max_size=100,100;pivot=0.5,0.5"
                    .to_owned(),
            );

        // 1) 字段行与控件：向量一律 DragValue 连排（与手写面板同级）。
        let rows = app.schema_field_rows();
        let widget = |field: &str| {
            let row = rows
                .iter()
                .find(|row| row.component == "UiLayout" && row.field == field)
                .unwrap_or_else(|| panic!("{field} 行"));
            field_widget(row.kind, row.field)
        };
        assert_eq!(
            widget("anchors"),
            FieldWidget::Vector {
                parts: 4,
                size: false
            }
        );
        assert_eq!(
            widget("offsets"),
            FieldWidget::Vector {
                parts: 4,
                size: false
            }
        );
        assert_eq!(
            widget("pivot"),
            FieldWidget::Vector {
                parts: 2,
                size: false
            }
        );
        assert_eq!(
            widget("max_size"),
            FieldWidget::Vector {
                parts: 2,
                size: false
            }
        );

        // 2) 组件级动作：12 个预设 + 选中态推导。
        let actions = app.schema_component_actions("UiLayout");
        assert_eq!(
            actions.len(),
            rustlua_ui::ui_layout::UiLayoutPreset::ALL.len()
        );
        assert!(
            actions.iter().all(|action| !action.selected),
            "默认布局不匹配任何预设（显示为自定义）"
        );

        // 3) 应用一个预设：一条 Undo、引擎读回、offsets 不变。
        let full_rect = actions
            .iter()
            .find(|action| action.label == rustlua_ui::ui_layout::UiLayoutPreset::FullRect.label())
            .expect("FullRect 动作")
            .clone();
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentFields {
                entity,
                component: "UiLayout".to_owned(),
                fields: full_rect.fields.clone(),
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1, "预设 = 一条 Undo");
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["UiLayout"]
            .clone();
        let layout = rustlua_ui::ui_layout::UiRectLayout::from_component_str(&payload).unwrap();
        assert_eq!(layout.anchors, [0.0, 0.0, 1.0, 1.0], "FullRect = 四边贴满");
        assert_eq!(layout.offsets, [0.0, 0.0, 0.0, 0.0], "未涉及的字段原样保留");
        // 应用后该预设成为选中态。
        let actions = app.schema_component_actions("UiLayout");
        assert_eq!(actions.iter().filter(|action| action.selected).count(), 1);

        // 4) anchors 越界仍被拒绝（原手写面板的 `0..=1` 校验没有丢——现在由引擎解析器拦）。
        match app.dispatch(EditorCommand::SetComponentField {
            entity,
            component: "UiLayout".to_owned(),
            field: "anchors".to_owned(),
            value: "1.5,0,0,0".to_owned(),
        }) {
            CommandResult::Rejected(reason) => assert!(!reason.is_empty(), "{reason}"),
            other => panic!("越界 anchor 必须被拒绝，实际 {other:?}"),
        }
        // 5) 默认值来自引擎（`default_element()`：中心锚 200×100）。
        let default_payload =
            rustlua_ui::ui_layout::UiRectLayout::default_element().to_component_str();
        let default_layout =
            rustlua_ui::ui_layout::UiRectLayout::from_component_str(&default_payload).unwrap();
        assert_eq!(default_layout.offsets, [-100.0, -50.0, 100.0, 50.0]);
    }

    /// **字段规则铺开**（第 32 轮）：纹理引用字段获得"工程内相对路径 + 图片扩展名"规则，
    /// 且**空值 = 未指定**放行（引擎自己的默认编码就是 `texture=`）。规则同时进入**审计**。
    #[test]
    fn texture_reference_rule_is_enforced_through_the_generic_editor() {
        let mut app = app_with_quad();
        let entity = SceneEntityId(1);
        // 合法：一次 Undo + 引擎能读回。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentField {
                entity,
                component: "Quad".to_owned(),
                field: "texture".to_owned(),
                value: "assets/textures/ui/panel.png".to_owned(),
            }),
            CommandResult::Applied
        );
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["Quad"]
            .clone();
        assert!(
            rustlua_render::quad::Quad::from_component_str(&payload).is_ok(),
            "引擎必须能读回：{payload}"
        );
        // 行提示里带着规则说明（用户能看到"为什么被拒绝"）。
        let hint = app
            .schema_field_rows()
            .into_iter()
            .find(|row| row.component == "Quad" && row.field == "texture")
            .expect("Quad.texture 行")
            .hint;
        assert!(hint.contains("png"), "{hint}");
        // 非法扩展名 / 穿越 / 绝对路径 / 非图片：拒绝且不产生历史。
        let before = app.history.undo_len();
        for bad in [
            "assets/textures/ui/panel.bmp",
            "../escape.png",
            "/etc/panel.png",
            "scripts/player.lua",
        ] {
            match app.dispatch(EditorCommand::SetComponentField {
                entity,
                component: "Quad".to_owned(),
                field: "texture".to_owned(),
                value: bad.to_owned(),
            }) {
                CommandResult::Rejected(reason) => assert!(!reason.is_empty(), "{bad}"),
                other => panic!("{bad} 必须被拒绝，实际 {other:?}"),
            }
        }
        assert_eq!(app.history.undo_len(), before, "拒绝不得产生历史");
        // 空值 = 未指定：`Quad` 的引擎解析器**明确拒绝**空纹理（`quad texture must not be empty`），
        // 因此这里也必须被拒绝——schema 与引擎同口径（`Sprite`/`Image` 才允许空）。
        match app.dispatch(EditorCommand::SetComponentField {
            entity,
            component: "Quad".to_owned(),
            field: "texture".to_owned(),
            value: String::new(),
        }) {
            CommandResult::Rejected(_) => {}
            other => panic!("Quad 的空纹理必须被拒绝（引擎读不了），实际 {other:?}"),
        }
    }

    /// **迁移证据**（第 36 轮，第 57 轮随统一收敛）：唯一的 `Transform` 走通用字段路径——
    /// 字段全是 DragValue、**重置**是组件级动作（值来自引擎 `identity()`），提交一条 Undo、
    /// 引擎能读回。等比锁开关已随统一撤掉（`scale` 是 `Vec3`，没有可复制的单一字段值）。
    #[test]
    fn migrated_transforms_keep_toggles_and_reset_actions() {
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
                "Transform".to_owned(),
                "position=3.000000000,4.000000000,0.000000000;rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=2.000000000,2.000000000,1.000000000".to_owned(),
            );

        // 字段行与控件：`position`/`scale` = Vec3、`rotation` = Vec4。
        let rows = app.schema_field_rows();
        for (component, field, parts) in [
            ("Transform", "position", 3_usize),
            ("Transform", "rotation", 4),
            ("Transform", "scale", 3),
        ] {
            let row = rows
                .iter()
                .find(|row| row.component == component && row.field == field)
                .unwrap_or_else(|| panic!("{component}.{field} 行"));
            let expected = if parts == 1 {
                FieldWidget::Number { integer: false }
            } else {
                FieldWidget::Vector { parts, size: false }
            };
            assert_eq!(field_widget(row.kind, row.field), expected);
        }
        // 第 56 轮：统一后 `scale` 是 `Vec3`，等比锁开关撤掉；"重置为默认值"动作保留。
        assert!(
            editor::component_toggles("Transform").is_empty(),
            "统一后的 Transform 不再有等比锁开关"
        );
        assert_eq!(app.schema_component_actions("Transform").len(), 1);

        // 组件级动作：一次提交写完 3 个字段 = 一条 Undo，结果等于引擎的单位变换。
        let reset = app.schema_component_actions("Transform")[0].fields.clone();
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentFields {
                entity,
                component: "Transform".to_owned(),
                fields: reset,
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1);
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["Transform"]
            .clone();
        assert_eq!(
            rustlua_ecs::transform::Transform3D::from_component_str(&payload).unwrap(),
            rustlua_ecs::transform::Transform3D::identity()
        );
        // 重置后动作列表变空（视图据此不显示按钮）。
        assert!(app.schema_component_actions("Transform").is_empty());

        // 字段提交仍是一条 Undo + 引擎读回。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentField {
                entity,
                component: "Transform".to_owned(),
                field: "scale".to_owned(),
                value: "2,2,2".to_owned(),
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 2);
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["Transform"]
            .clone();
        assert_eq!(
            rustlua_ecs::transform::Transform3D::from_component_str(&payload)
                .unwrap()
                .scale,
            rustlua_math::math3d::Vec3::new(2.0, 2.0, 2.0)
        );
        // 缩放为 0 仍被拒绝（引擎解析器的规则，经兜底门生效）。
        match app.dispatch(EditorCommand::SetComponentField {
            entity,
            component: "Transform".to_owned(),
            field: "scale".to_owned(),
            value: "0,1,1".to_owned(),
        }) {
            CommandResult::Rejected(_) => {}
            other => panic!("零缩放必须被拒绝，实际 {other:?}"),
        }
    }

    /// **迁移证据**（第 34 轮）：`Sprite` 走通用字段路径——纹理 = 文本框（带图片扩展名规则）、
    /// 尺寸 = `宽x高`、`pivot`/`tint` = DragValue/取色器、**排序层 = 引擎已知层名下拉**、
    /// `order` = DragValue、`flip_x/flip_y` = 勾选框（与手写面板同级或更好）。
    #[test]
    fn migrated_sprite_keeps_widgets_and_sorting_layer_choices() {
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

        let rows = app.schema_field_rows();
        let widget = |field: &str| {
            let row = rows
                .iter()
                .find(|row| row.component == "Sprite" && row.field == field)
                .unwrap_or_else(|| panic!("{field} 行"));
            field_widget(row.kind, row.field)
        };
        assert_eq!(widget("texture"), FieldWidget::Text);
        assert_eq!(
            widget("size"),
            FieldWidget::Vector {
                parts: 2,
                size: true
            }
        );
        assert_eq!(
            widget("pivot"),
            FieldWidget::Vector {
                parts: 2,
                size: false
            }
        );
        assert_eq!(widget("tint"), FieldWidget::Color { parts: 4 });
        assert_eq!(widget("flip_x"), FieldWidget::Checkbox);
        assert_eq!(widget("order"), FieldWidget::Number { integer: true });
        // 排序层：候选来自**引擎已知层名**（与旧手写面板的下拉一致）。
        let layer = rows
            .iter()
            .find(|row| row.component == "Sprite" && row.field == "sorting_layer")
            .expect("Sprite.sorting_layer 行");
        assert_eq!(layer.choices_source, editor::ChoiceSource::SortingLayers);
        assert_eq!(field_widget(layer.kind, layer.field), FieldWidget::Choice);
        assert_eq!(layer.choices, editor::SORTING_LAYERS);
        // 纹理提示里带着扩展名规则（迁移前由 `validate_asset_reference` 拦的同一口径）。
        let texture = rows
            .iter()
            .find(|row| row.component == "Sprite" && row.field == "texture")
            .expect("Sprite.texture 行");
        assert!(texture.hint.contains("png"), "{}", texture.hint);

        // 提交排序层：一条 Undo + 引擎读回。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentField {
                entity,
                component: "Sprite".to_owned(),
                field: "sorting_layer".to_owned(),
                value: "actor".to_owned(),
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1);
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["Sprite"]
            .clone();
        let sprite = rustlua_render::sprite::SpriteComponent::from_component_str(&payload)
            .expect("引擎解析器");
        assert_eq!(sprite.sorting.layer, "actor");
        assert_eq!(sprite.texture, "assets/textures/player.png", "其它字段不动");

        // 纹理规则同时生效（迁移前由编辑器拦，现在由 schema 规则拦且进入审计）。
        match app.dispatch(EditorCommand::SetComponentField {
            entity,
            component: "Sprite".to_owned(),
            field: "texture".to_owned(),
            value: "assets/x.bmp".to_owned(),
        }) {
            CommandResult::Rejected(_) => {}
            other => panic!("非图片扩展名必须被拒绝，实际 {other:?}"),
        }
        assert_eq!(app.history.undo_len(), 1, "拒绝不得产生历史");
    }

    /// **多选批量 + 混合值**（第 33 轮）：把通用编辑器补到与手写面板同级——一次编辑写到
    /// **所有选中且拥有该组件**的实体上（**一条** Undo），取值不同的字段显示"混合"。
    #[test]
    fn multi_selection_edit_writes_all_targets_in_one_undo() {
        let mut app = app_with_quad();
        let (a, b, c) = (SceneEntityId(1), SceneEntityId(2), SceneEntityId(3));
        {
            let scene = &mut app.document.current_scene.as_mut().unwrap().scene;
            for id in [b, c] {
                scene.add_entity(id, "Peer", None).unwrap();
            }
            // b 有 Quad（取值不同），c **没有** Quad（不该被写、也不参与混合判定）。
            scene
                .entities
                .get_mut(&b)
                .unwrap()
                .components
                .insert("Quad".to_owned(), "size=8,8;color=1,1,1,1".to_owned());
        }
        // 选择顺序：最后加入的成为主选中 → 先选 b、c，最后选 a（a 有 Quad，作为基准）。
        app.selection
            .set_single(b, editor::SelectionSource::Hierarchy);
        app.selection.add(c, editor::SelectionSource::Hierarchy);
        app.selection.add(a, editor::SelectionSource::Hierarchy);
        assert_eq!(app.selection.primary(), Some(a));
        // 真实 UI 每帧都会把 Inspector 缓冲与文档对齐；换了选区之后也要对齐，
        // 否则 `Undo` 的帧末收敛边界会先把"未同步的缓冲"提交成一条历史（与 app_with_quad 同理）。
        app.sync_inspector(a);

        // 1) 混合值：size 在 a/b 上不同 → mixed；c 没有该组件不参与。
        let rows = app.schema_field_rows();
        let size = rows
            .iter()
            .find(|row| row.component == "Quad" && row.field == "size")
            .expect("Quad.size 行");
        assert!(size.mixed, "a(64,32) 与 b(8,8) 的 size 不同 → 必须显示混合");
        let color = rows
            .iter()
            .find(|row| row.component == "Quad" && row.field == "color")
            .expect("Quad.color 行");
        assert!(!color.mixed, "a 与 b 的 color 相同 → 不是混合");

        // 2) 提交：两个目标都写、一条 Undo、c 不动。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentFieldOnSelection {
                component: "Quad".to_owned(),
                field: "size".to_owned(),
                value: "128,64".to_owned(),
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1, "多选批量 = 一条 Undo");
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        for id in [a, b] {
            assert!(
                scene.entities[&id].components["Quad"].contains("size=128.000000,64.000000"),
                "实体 {} 必须被写入：{}",
                id.0,
                scene.entities[&id].components["Quad"]
            );
        }
        assert!(
            !scene.entities[&c].components.contains_key("Quad"),
            "没有该组件的实体不动"
        );

        // 3) 全部目标都已等于该取值 → NoOp（不产生新历史）。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentFieldOnSelection {
                component: "Quad".to_owned(),
                field: "size".to_owned(),
                value: "128,64".to_owned(),
            }),
            CommandResult::NoOp
        );
        assert_eq!(app.history.undo_len(), 1);

        // 4) 非法取值：整体不写（a、b 都保持上一步的值）且不产生历史。
        match app.dispatch(EditorCommand::SetComponentFieldOnSelection {
            component: "Quad".to_owned(),
            field: "size".to_owned(),
            value: "64".to_owned(),
        }) {
            CommandResult::Rejected(_) => {}
            other => panic!("非法批量取值必须被拒绝，实际 {other:?}"),
        }
        assert_eq!(app.history.undo_len(), 1);

        // 5) Undo 一步把**所有目标**一起回滚（比较引擎解析出的取值，避免文本格式差异）。
        assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let size_of = |id: SceneEntityId| {
            rustlua_render::quad::Quad::from_component_str(&scene.entities[&id].components["Quad"])
                .expect("引擎解析器")
                .size
        };
        assert_eq!(size_of(a), rustlua_ecs::transform::Vec2::new(64.0, 32.0));
        assert_eq!(size_of(b), rustlua_ecs::transform::Vec2::new(8.0, 8.0));
    }

    /// **原子多字段提交**（第 30 轮）：组件级动作（如锚点预设）一次改多个字段，
    /// 必须只有**一条** Undo；任一步非法则整体不写。
    #[test]
    fn multi_field_action_edit_is_one_undo_and_atomic() {
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
                "UiLayout".to_owned(),
                "anchors=0,0,0,0;offsets=1,2,3,4;min_size=0,0;max_size=100,100;pivot=0.5,0.5"
                    .to_owned(),
            );

        // 一次提交改两个字段 → 恰好一条 Undo。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentFields {
                entity,
                component: "UiLayout".to_owned(),
                fields: vec![
                    ("anchors".to_owned(), "0.5,0.5,0.5,0.5".to_owned()),
                    ("pivot".to_owned(), "0.5,0.5".to_owned()),
                ],
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1, "多字段提交必须只有一条历史");
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["UiLayout"]
            .clone();
        let layout =
            rustlua_ui::ui_layout::UiRectLayout::from_component_str(&payload).expect("引擎解析器");
        assert_eq!(layout.anchors, [0.5, 0.5, 0.5, 0.5]);
        assert_eq!(
            layout.offsets,
            [1.0, 2.0, 3.0, 4.0],
            "未涉及的字段必须原样保留"
        );

        // 原子性：第二个字段非法 → 整体不写、文档与历史都不变。
        let before = app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
            .components["UiLayout"]
            .clone();
        match app.dispatch(EditorCommand::SetComponentFields {
            entity,
            component: "UiLayout".to_owned(),
            fields: vec![
                ("anchors".to_owned(), "1,1,1,1".to_owned()),
                ("pivot".to_owned(), "不是数字".to_owned()),
            ],
        }) {
            CommandResult::Rejected(_) => {}
            other => panic!("非法多字段提交必须被拒绝，实际 {other:?}"),
        }
        assert_eq!(app.history.undo_len(), 1, "拒绝不得产生历史");
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.entities[&entity].components["UiLayout"],
            before,
            "任一步失败必须整体不写"
        );

        // 幂等：同一组取值再提交一次是 NoOp。
        assert_eq!(
            app.dispatch(EditorCommand::SetComponentFields {
                entity,
                component: "UiLayout".to_owned(),
                fields: vec![
                    ("anchors".to_owned(), "0.5,0.5,0.5,0.5".to_owned()),
                    ("pivot".to_owned(), "0.5,0.5".to_owned()),
                ],
            }),
            CommandResult::NoOp
        );
        assert_eq!(app.history.undo_len(), 1);

        // 组件级动作的列表来自引擎预设（视图渲染用的就是它）。
        let actions = app.schema_component_actions("UiLayout");
        assert_eq!(
            actions.len(),
            rustlua_ui::ui_layout::UiLayoutPreset::ALL.len()
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
        assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
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
        app.play.set_state_for_test(editor::PlayState::Playing);
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

    // ---- M27-01：资源引用字段的落点结算 ----

    /// 三种结算各管各的：自家资产**只交出一个值**（写文档仍走 `pending` 那条 dispatch）；
    /// 工程外的文件**先入库**再交出相对路径；拒绝没有值、只把原因写进状态栏，磁盘与文档都不动。
    #[test]
    fn a_reference_field_drop_yields_a_value_or_a_reason() {
        use crate::dnd::DropAction;
        let mut app = app_with_quad();
        assert_eq!(
            app.commit_field_ref_drop(
                &DropAction::SetAssetRef("assets/textures/a.png".to_owned()),
                String::new(),
            )
            .as_deref(),
            Some("assets/textures/a.png")
        );

        let name = format!("rustlua-inspector-drop-{}.png", crate::util::test_token());
        let outside = std::env::temp_dir().join(&name);
        std::fs::write(&outside, b"png").expect("写外部文件");
        let value =
            app.commit_field_ref_drop(&DropAction::ImportAndSetRef(outside.clone()), String::new());
        assert_eq!(
            value.as_deref(),
            Some(format!("assets/textures/{name}").as_str()),
            "外部文件要先复制进家族目录，字段写的是工程相对路径：{}",
            app.status
        );
        assert!(
            app.project
                .root
                .join("assets/textures")
                .join(&name)
                .is_file(),
            "文件必须真的入库"
        );
        assert!(
            app.status.starts_with("已导入 1 个文件"),
            "状态={}",
            app.status
        );

        let entities = app
            .document
            .current_scene
            .as_ref()
            .unwrap()
            .scene
            .entities
            .len();
        assert_eq!(
            app.commit_field_ref_drop(
                &DropAction::Reject("这个字段要的是纹理/模型/预制体".to_owned()),
                "（跳过 1 个链接）".to_owned(),
            ),
            None
        );
        assert_eq!(
            app.status, "这个字段要的是纹理/模型/预制体（跳过 1 个链接）",
            "拒绝必须带上损耗说明"
        );
        assert_eq!(
            app.document
                .current_scene
                .as_ref()
                .unwrap()
                .scene
                .entities
                .len(),
            entities
        );
        assert_eq!(app.history.undo_len(), 0, "落点结算本身不写历史");
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&app.project.root);
    }
}
