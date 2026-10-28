//! Inspector 的 UI 子编辑器：事件 / 绑定 / 按钮态 / UI 动画四块面板的绘制。
//!
//! 从 `panels/inspector.rs` 原样搬出（P1 结构收口）；方法仍是 `impl EditorApp`，
//! 只是把四个彼此独立的绘制段从 3k 行文件里分出来，无行为变化。

use super::helpers::*;
use super::*;

impl EditorApp {
    pub(super) fn draw_ui_events_editor(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("界面事件");
        let mut changed = false;
        for index in 0..self.events_editor.bindings.len() {
            let mut binding = self.events_editor.bindings[index].clone();
            egui::Grid::new(format!("ui_event_{index}")).show(ui, |ui| {
                let mut event = binding
                    .event
                    .unwrap_or(rustlua_ui::ui_events::UiEventType::Click);
                if egui::ComboBox::from_id_salt(format!("ui_event_type_{index}"))
                    .selected_text(ui_event_label(event))
                    .show_ui(ui, |ui| {
                        for candidate in [
                            rustlua_ui::ui_events::UiEventType::Click,
                            rustlua_ui::ui_events::UiEventType::PointerEnter,
                            rustlua_ui::ui_events::UiEventType::PointerExit,
                            rustlua_ui::ui_events::UiEventType::PointerDown,
                            rustlua_ui::ui_events::UiEventType::PointerUp,
                            rustlua_ui::ui_events::UiEventType::Focus,
                            rustlua_ui::ui_events::UiEventType::Submit,
                            rustlua_ui::ui_events::UiEventType::ValueChanged,
                            rustlua_ui::ui_events::UiEventType::Drag,
                        ] {
                            ui.selectable_value(&mut event, candidate, ui_event_label(candidate));
                        }
                    })
                    .response
                    .changed()
                {
                    binding.event = Some(event);
                    changed = true;
                }
                if ui
                    .add(
                        egui::TextEdit::singleline(binding.target.get_or_insert_with(String::new))
                            .hint_text("目标"),
                    )
                    .changed()
                {
                    if binding.target.as_deref().is_none_or(str::is_empty) {
                        binding.target = None;
                    }
                    changed = true;
                }
                if ui
                    .add(egui::TextEdit::singleline(&mut binding.function).hint_text("函数"))
                    .changed()
                {
                    changed = true;
                }
                let mut args = binding
                    .arguments
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join(",");
                if ui
                    .add(egui::TextEdit::singleline(&mut args).hint_text("键=值,键2=值2"))
                    .changed()
                {
                    binding.arguments = args
                        .split([',', ';'])
                        .filter_map(|item| item.trim().split_once('='))
                        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
                        .filter(|(key, _)| !key.is_empty())
                        .collect();
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                if ui.button("移除").clicked() {
                    self.events_editor.bindings.remove(index);
                    changed = true;
                }
            });
            if changed {
                self.events_editor.bindings[index] = binding;
            }
        }
        ui.horizontal(|ui| {
            if ui.button("添加事件").clicked() {
                self.events_editor
                    .bindings
                    .push(rustlua_ui::ui_events::UiEventBinding {
                        event: Some(rustlua_ui::ui_events::UiEventType::Click),
                        target: None,
                        function: "on_click".to_owned(),
                        arguments: Default::default(),
                    });
            }
            if ui.button("应用事件").clicked() {
                self.apply_events_editor();
            }
        });
    }

    pub(super) fn draw_ui_bindings_editor(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("属性绑定");
        let mut changed = false;
        for index in 0..self.bindings_editor.bindings.len() {
            let mut binding = self.bindings_editor.bindings[index].clone();
            egui::Grid::new(format!("ui_binding_{index}")).show(ui, |ui| {
                let property = binding
                    .property
                    .unwrap_or(rustlua_ui::ui_binding::BoundProperty::TextText);
                if egui::ComboBox::from_id_salt(format!("binding_property_{index}"))
                    .selected_text(bound_property_label(property))
                    .show_ui(ui, |ui| {
                        for candidate in [
                            rustlua_ui::ui_binding::BoundProperty::TextText,
                            rustlua_ui::ui_binding::BoundProperty::ImageVisible,
                            rustlua_ui::ui_binding::BoundProperty::ProgressValue,
                        ] {
                            ui.selectable_value(
                                &mut binding.property,
                                Some(candidate),
                                bound_property_label(candidate),
                            );
                        }
                    })
                    .response
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .add(egui::TextEdit::singleline(&mut binding.target).hint_text("目标"))
                    .changed()
                {
                    changed = true;
                }
                let mut source = binding.source.0.join(".");
                if ui
                    .add(egui::TextEdit::singleline(&mut source).hint_text("数据路径"))
                    .changed()
                    && let Ok(path) = rustlua_ui::ui_binding::BindingPath::parse(&source)
                {
                    binding.source = path;
                    changed = true;
                }
                let mut default = binding.default.clone().unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut default).hint_text("默认值"))
                    .changed()
                {
                    binding.default = (!default.trim().is_empty()).then_some(default);
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                if ui.button("移除").clicked() {
                    self.bindings_editor.bindings.remove(index);
                    changed = true;
                }
            });
            if changed {
                self.bindings_editor.bindings[index] = binding;
            }
        }
        ui.horizontal(|ui| {
            if ui.button("添加绑定").clicked() {
                self.bindings_editor
                    .bindings
                    .push(rustlua_ui::ui_binding::PropertyBinding {
                        property: Some(rustlua_ui::ui_binding::BoundProperty::TextText),
                        target: String::new(),
                        source: rustlua_ui::ui_binding::BindingPath(vec![
                            "data".to_owned(),
                            "value".to_owned(),
                        ]),
                        default: None,
                        flow: rustlua_ui::ui_binding::BindingFlow::OneWay,
                    });
            }
            if ui.button("应用绑定").clicked() {
                self.apply_bindings_editor();
            }
        });
    }

    pub(super) fn draw_button_states_editor(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("按钮状态");
        let states = &mut self.button_states_editor;
        for (label, value) in [
            ("正常", &mut states.normal),
            ("悬停", &mut states.hover),
            ("按下", &mut states.pressed),
            ("禁用", &mut states.disabled),
            ("聚焦", &mut states.focused),
        ] {
            let mut text = value.clone().unwrap_or_default();
            if ui
                .add(egui::TextEdit::singleline(&mut text).hint_text(label))
                .changed()
            {
                *value = (!text.trim().is_empty()).then(|| text.trim().to_owned());
            }
        }
        if ui
            .checkbox(&mut self.button_states_editor.disabled_state, "禁用状态")
            .changed()
        {
            self.status = "按钮状态已在缓冲中修改".to_owned();
        }
        if ui.button("应用按钮状态").clicked() {
            self.apply_button_states_editor();
        }
    }

    pub(super) fn draw_ui_animation_editor(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("界面动画");
        for (property, track) in self.ui_animation_editor.tracks.clone() {
            ui.label(format!("轨道：{}", ui_animation_property_label(&property)));
            for (index, mut keyframe) in track.keyframes.into_iter().enumerate() {
                ui.horizontal(|ui| {
                    let animation_changed = ui
                        .add(egui::DragValue::new(&mut keyframe.time).speed(0.01))
                        .changed()
                        || ui
                            .add(egui::DragValue::new(&mut keyframe.value).speed(0.01))
                            .changed();
                    if animation_changed
                        && let Some(track) = self.ui_animation_editor.tracks.get_mut(&property)
                    {
                        track.keyframes[index] = keyframe;
                    }
                    let mut easing = keyframe.easing;
                    egui::ComboBox::from_id_salt(format!("animation_easing_{property}_{index}"))
                        .selected_text(ui_easing_label(easing))
                        .show_ui(ui, |ui| {
                            for candidate in
                                [UiEasing::Linear, UiEasing::Step, UiEasing::SmoothStep]
                            {
                                ui.selectable_value(
                                    &mut easing,
                                    candidate,
                                    ui_easing_label(candidate),
                                );
                            }
                        });
                    if easing != keyframe.easing
                        && let Some(track) = self.ui_animation_editor.tracks.get_mut(&property)
                    {
                        track.keyframes[index].easing = easing;
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui.button("加关键帧").clicked()
                    && let Some(track) = self.ui_animation_editor.tracks.get_mut(&property)
                {
                    let time = track.keyframes.last().map_or(0.0, |frame| frame.time + 0.5);
                    track.keyframes.push(UiKeyframe {
                        time,
                        value: 0.0,
                        easing: UiEasing::Linear,
                    });
                }
            });
        }
        ui.horizontal(|ui| {
            if ui.button("添加不透明度轨道").clicked() {
                let track = rustlua_ui::ui_animation::UiAnimationTrack::new(
                    "opacity",
                    vec![UiKeyframe {
                        time: 0.0,
                        value: 0.0,
                        easing: UiEasing::SmoothStep,
                    }],
                );
                if let Ok(track) = track {
                    let _ = self.ui_animation_editor.add_track(track);
                }
            }
            if ui.button("播放").clicked() && !self.ui_animation_playing {
                self.toggle_ui_animation_preview();
            }
            if ui.button("停止").clicked() && self.ui_animation_playing {
                self.toggle_ui_animation_preview();
            }
            if ui.button("归零").clicked() {
                self.reset_ui_animation_preview();
            }
            if ui.button("应用动画").clicked() {
                self.apply_ui_animation_editor();
            }
        });
        if let Some(preview) = &self.ui_animation_preview {
            ui.label(format!("预览 t={:.2}", preview.time));
        }
    }
}
