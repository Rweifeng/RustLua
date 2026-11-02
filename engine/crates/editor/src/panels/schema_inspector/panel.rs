use super::*;

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
    pub(crate) fn schema_edit_targets(&self, component: &str) -> usize {
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
                ui.label(format!(
                    "▸ {}",
                    crate::util::component_display_name(row.component)
                ));
                // 编辑器态开关（不写文档）：如 Transform/Transform3D 的"锁定比例"。
                let mut open_toggles = Vec::new();
                for toggle in editor::component_toggles(row.component) {
                    let key = egui::Id::new(("schema_toggle", selected, toggle.key));
                    let mut on = ui
                        .data_mut(|data| *data.get_temp_mut_or_insert_with(key, || toggle.default));
                    ui.add(egui::Checkbox::new(&mut on, toggle.label));
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
                        ui.scope(|ui| {
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
            // P1：Play 中**取值可以改**（Unity 式运行中调参）——写入的是运行态镜像，
            // Stop 默认丢弃，点工具栏「应用到场景」才进编辑文档（见 shell 的
            // `crate::actions::apply_play_field_edit`）。
            // 唯一仍然锁着的是**资源引用类**字段：预览里的贴图/网格不会因为镜像换了路径而
            // 重新加载，放开编辑只会让"改了看着生效、其实什么都没动"。
            let ref_locked = playing && row.path_rule;
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
                        let response = ui.add(egui::Checkbox::without_text(&mut flag));
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
                        let response = ui.add_enabled_ui(!ref_locked, |ui| {
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
                            .add(
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
                            let drag =
                                ui.add(egui::DragValue::new(value).speed(0.1).max_decimals(4));
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
                        let response = ui.scope(|ui| {
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
                                !ref_locked,
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
    pub(crate) fn inspector_field_drop(
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
                self.status = format!("播放模式下不能编辑场景，请先点击停止{}", drop.notes());
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
    pub(crate) fn commit_field_ref_drop(
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
