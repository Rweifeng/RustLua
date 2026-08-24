//! 行3 属性面板（Inspector）：实体层级/命名、组件管理、Transform/Camera/Light/Collider 编辑。

use crate::ComponentClipboard;
use crate::EditorApp;
use editor::{
    ButtonFields, Camera3DFields, Collider3dFields, CommandKind, ImageFields, InspectorError,
    LightFields, TextFields, Transform3DFields, TransformFields,
};
use eframe::egui;
use engine::core::scene::SceneEntityId;

impl EditorApp {
    pub(crate) fn draw_inspector_panel(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(340.0);
        // 内容超出面板高度时可滚动。
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.heading("Inspector");
                let Some(selected) = self.tree.selected() else {
                    ui.label("请先在 Hierarchy 中选择实体");
                    return;
                };
                let Some((parent, child_count)) = self.current_scene().and_then(|scene| {
                    scene
                        .scene
                        .entities
                        .get(&selected)
                        .map(|e| (e.parent, e.children.len()))
                }) else {
                    ui.label("实体不存在");
                    return;
                };
                ui.label(format!("实体 ID：{}", selected.0));
                ui.label(format!(
                    "父实体：{}",
                    parent.map_or_else(|| "无".to_owned(), |id| id.0.to_string())
                ));
                ui.label(format!("子实体数量：{child_count}"));
                // 持久实体：运行时切场景不销毁。
                {
                    let mut persistent = self
                        .current_scene()
                        .and_then(|scene| {
                            scene.scene.entities.get(&selected).map(|entity| {
                                entity
                                    .metadata
                                    .get("persistent")
                                    .is_some_and(|value| value == "true")
                            })
                        })
                        .unwrap_or(false);
                    if ui
                        .checkbox(&mut persistent, "运行时切场景不销毁（持久实体）")
                        .changed()
                    {
                        let inspector = self.inspector.clone();
                        if let Some(scene) = self.document.current_scene.as_mut() {
                            let _ = self.history.execute(
                                scene,
                                "Toggle Persistent",
                                CommandKind::Property,
                                |scene| {
                                    inspector.set_persistent(scene, persistent)?;
                                    Ok::<(), InspectorError>(())
                                },
                            );
                        }
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("父实体 ID");
                    ui.text_edit_singleline(&mut self.parent_entity);
                    if ui.button("设置父级").clicked() {
                        let parent = self
                            .parent_entity
                            .trim()
                            .parse::<u64>()
                            .ok()
                            .map(SceneEntityId);
                        self.reparent_selected(parent);
                    }
                });
                if ui.button("解除父级").clicked() {
                    self.parent_entity.clear();
                    self.reparent_selected(None);
                }
                ui.horizontal(|ui| {
                    ui.label("名称");
                    ui.text_edit_singleline(&mut self.entity_name);
                    if ui.button("重新命名").clicked() {
                        self.rename_selected();
                    }
                });
                // M10-06：组件管理（添加/移除）。
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("添加组件");
                    egui::ComboBox::from_id_salt("inspector_add_component")
                        .selected_text(&self.inspector_add_component)
                        .show_ui(ui, |ui| {
                            for name in [
                                "Transform3D",
                                "MeshRenderer",
                                "Camera3D",
                                "Light",
                                "Collider3d",
                                "Text",
                                "Image",
                                "Button",
                            ] {
                                ui.selectable_value(
                                    &mut self.inspector_add_component,
                                    name.to_owned(),
                                    name,
                                );
                            }
                        });
                    if ui.button("添加").clicked() {
                        let component = self.inspector_add_component.clone();
                        if let Err(error) = self.add_component(&component) {
                            self.status = error;
                        }
                    }
                });
                let components = self.current_components();
                // 只显示实体实际拥有的组件编辑器（空 2D 实体只有 Transform，3D 只有 Transform3D）。
                let has = |name: &str| components.iter().any(|component| component == name);
                if !components.is_empty() {
                    ui.label("组件：");
                    for key in &components {
                        ui.horizontal(|ui| {
                            ui.label(key);
                            if ui.small_button("移除").clicked() {
                                let key = key.clone();
                                if let Err(error) = self.remove_component(&key) {
                                    self.status = error;
                                }
                            }
                        });
                    }
                }
                ui.separator();
                if has("Transform") {
                    let transform_header = if self.inspector_transform_open {
                        "Transform  v"
                    } else {
                        "Transform  >"
                    };
                    if ui.button(transform_header).clicked() {
                        self.inspector_transform_open = !self.inspector_transform_open;
                    }
                    if self.inspector_transform_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("位置 X");
                                ui.add(egui::DragValue::new(&mut self.transform.position_x));
                            });
                            ui.horizontal(|ui| {
                                ui.label("位置 Y");
                                ui.add(egui::DragValue::new(&mut self.transform.position_y));
                            });
                            ui.horizontal(|ui| {
                                ui.label("旋转");
                                ui.add(egui::DragValue::new(&mut self.transform.rotation));
                            });
                            ui.horizontal(|ui| {
                                ui.label("缩放 X");
                                ui.add(egui::DragValue::new(&mut self.transform.scale_x));
                            });
                            ui.horizontal(|ui| {
                                ui.label("缩放 Y");
                                ui.add(egui::DragValue::new(&mut self.transform.scale_y));
                            });
                            ui.horizontal(|ui| {
                                if ui.button("Reset").clicked() {
                                    self.transform = TransformFields::default();
                                }
                                if ui.button("Copy Components").clicked() {
                                    self.copy_selected_components();
                                }
                                if ui.button("Paste Components").clicked() {
                                    self.paste_selected_components();
                                }
                            });
                            if ui.button("Apply Transform").clicked() {
                                self.apply_inspector();
                            }
                        });
                    }
                }
                if has("Transform3D") {
                    let transform3d_header = if self.inspector_transform3d_open {
                        "Transform3D  v"
                    } else {
                        "Transform3D  >"
                    };
                    if ui.button(transform3d_header).clicked() {
                        self.inspector_transform3d_open = !self.inspector_transform3d_open;
                    }
                    if self.inspector_transform3d_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("位置");
                                ui.add(egui::DragValue::new(&mut self.transform3d.position_x));
                                ui.add(egui::DragValue::new(&mut self.transform3d.position_y));
                                ui.add(egui::DragValue::new(&mut self.transform3d.position_z));
                            });
                            ui.horizontal(|ui| {
                                ui.label("旋转 Q");
                                ui.add(egui::DragValue::new(&mut self.transform3d.rotation_x));
                                ui.add(egui::DragValue::new(&mut self.transform3d.rotation_y));
                                ui.add(egui::DragValue::new(&mut self.transform3d.rotation_z));
                                ui.add(egui::DragValue::new(&mut self.transform3d.rotation_w));
                            });
                            ui.horizontal(|ui| {
                                ui.label("缩放");
                                ui.add(egui::DragValue::new(&mut self.transform3d.scale_x));
                                ui.add(egui::DragValue::new(&mut self.transform3d.scale_y));
                                ui.add(egui::DragValue::new(&mut self.transform3d.scale_z));
                            });
                            ui.horizontal(|ui| {
                                if ui.button("Reset 3D").clicked() {
                                    self.transform3d = Transform3DFields::default();
                                }
                            });
                        });
                    }
                }
                if has("MeshRenderer") {
                    ui.horizontal(|ui| {
                        ui.label("MeshRenderer");
                        ui.text_edit_singleline(&mut self.mesh_renderer);
                    });
                }
                if has("Camera3D") {
                    ui.group(|ui| {
                        ui.label("Camera3D");
                        ui.horizontal(|ui| {
                            ui.label("位置");
                            ui.add(egui::DragValue::new(&mut self.camera3d.position_x));
                            ui.add(egui::DragValue::new(&mut self.camera3d.position_y));
                            ui.add(egui::DragValue::new(&mut self.camera3d.position_z));
                        });
                        ui.horizontal(|ui| {
                            ui.label("目标");
                            ui.add(egui::DragValue::new(&mut self.camera3d.target_x));
                            ui.add(egui::DragValue::new(&mut self.camera3d.target_y));
                            ui.add(egui::DragValue::new(&mut self.camera3d.target_z));
                        });
                        ui.horizontal(|ui| {
                            ui.label("FOV");
                            ui.add(egui::DragValue::new(&mut self.camera3d.fov_degrees));
                            ui.label("近/远");
                            ui.add(egui::DragValue::new(&mut self.camera3d.near));
                            ui.add(egui::DragValue::new(&mut self.camera3d.far));
                        });
                    });
                }
                if has("Light") {
                    ui.group(|ui| {
                        ui.label("Light");
                        ui.horizontal(|ui| {
                            ui.label("方向");
                            ui.add(egui::DragValue::new(&mut self.light.direction_x));
                            ui.add(egui::DragValue::new(&mut self.light.direction_y));
                            ui.add(egui::DragValue::new(&mut self.light.direction_z));
                        });
                        ui.horizontal(|ui| {
                            ui.label("颜色");
                            ui.add(egui::DragValue::new(&mut self.light.color_r));
                            ui.add(egui::DragValue::new(&mut self.light.color_g));
                            ui.add(egui::DragValue::new(&mut self.light.color_b));
                        });
                        ui.horizontal(|ui| {
                            ui.label("强度");
                            ui.add(egui::DragValue::new(&mut self.light.intensity));
                        });
                    });
                }
                if has("Collider3d") {
                    let collider3d_header = if self.inspector_collider3d_open {
                        "Collider3d  v"
                    } else {
                        "Collider3d  >"
                    };
                    if ui.button(collider3d_header).clicked() {
                        self.inspector_collider3d_open = !self.inspector_collider3d_open;
                    }
                    if self.inspector_collider3d_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("形状");
                                egui::ComboBox::from_id_salt("collider3d_shape")
                                    .selected_text(&self.collider3d.shape)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.collider3d.shape,
                                            "ball".to_owned(),
                                            "Ball",
                                        );
                                        ui.selectable_value(
                                            &mut self.collider3d.shape,
                                            "cuboid".to_owned(),
                                            "Cuboid",
                                        );
                                        ui.selectable_value(
                                            &mut self.collider3d.shape,
                                            "capsule".to_owned(),
                                            "Capsule",
                                        );
                                    });
                                ui.checkbox(&mut self.collider3d.sensor, "sensor");
                                ui.checkbox(&mut self.collider3d.dynamic, "dynamic");
                            });
                            match self.collider3d.shape.as_str() {
                                "cuboid" => {
                                    ui.horizontal(|ui| {
                                        ui.label("半宽/半高/半深");
                                        ui.add(egui::DragValue::new(
                                            &mut self.collider3d.half_width,
                                        ));
                                        ui.add(egui::DragValue::new(
                                            &mut self.collider3d.half_height,
                                        ));
                                        ui.add(egui::DragValue::new(&mut self.collider3d.half_z));
                                    });
                                }
                                "capsule" => {
                                    ui.horizontal(|ui| {
                                        ui.label("半高/半径");
                                        ui.add(egui::DragValue::new(
                                            &mut self.collider3d.half_height,
                                        ));
                                        ui.add(egui::DragValue::new(&mut self.collider3d.radius));
                                    });
                                }
                                _ => {
                                    ui.horizontal(|ui| {
                                        ui.label("半径");
                                        ui.add(egui::DragValue::new(&mut self.collider3d.radius));
                                    });
                                }
                            }
                            ui.horizontal(|ui| {
                                ui.label("偏移");
                                ui.add(egui::DragValue::new(&mut self.collider3d.offset_x));
                                ui.add(egui::DragValue::new(&mut self.collider3d.offset_y));
                                ui.add(egui::DragValue::new(&mut self.collider3d.offset_z));
                            });
                            ui.horizontal(|ui| {
                                if ui.button("添加 Collider3d").clicked()
                                    && let Err(error) = self.add_collider3d_component()
                                {
                                    self.status = error;
                                }
                                if ui.button("移除 Collider3d").clicked()
                                    && let Err(error) = self.remove_collider3d_component()
                                {
                                    self.status = error;
                                }
                            });
                        });
                    }
                }
                // UI 组件：Text / Image / Button（屏幕空间）。
                if has("Text") {
                    let text_header = if self.inspector_text_open {
                        "Text  v"
                    } else {
                        "Text  >"
                    };
                    if ui.button(text_header).clicked() {
                        self.inspector_text_open = !self.inspector_text_open;
                    }
                    if self.inspector_text_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("文本");
                                ui.text_edit_singleline(&mut self.text.text);
                            });
                            ui.horizontal(|ui| {
                                ui.label("位置");
                                ui.add(egui::DragValue::new(&mut self.text.x));
                                ui.add(egui::DragValue::new(&mut self.text.y));
                                ui.label("缩放");
                                ui.add(egui::DragValue::new(&mut self.text.scale).speed(0.1));
                            });
                            ui.horizontal(|ui| {
                                ui.label("颜色");
                                ui.add(
                                    egui::DragValue::new(&mut self.text.color_r).range(0.0..=1.0),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.text.color_g).range(0.0..=1.0),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.text.color_b).range(0.0..=1.0),
                                );
                            });
                            if ui.button("移除 Text").clicked()
                                && let Err(error) = self.remove_component("Text")
                            {
                                self.status = error;
                            }
                        });
                    }
                }
                if has("Image") {
                    let image_header = if self.inspector_image_open {
                        "Image  v"
                    } else {
                        "Image  >"
                    };
                    if ui.button(image_header).clicked() {
                        self.inspector_image_open = !self.inspector_image_open;
                    }
                    if self.inspector_image_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("纹理");
                                ui.text_edit_singleline(&mut self.image.texture);
                            });
                            ui.horizontal(|ui| {
                                ui.label("位置/尺寸");
                                ui.add(egui::DragValue::new(&mut self.image.x));
                                ui.add(egui::DragValue::new(&mut self.image.y));
                                ui.add(egui::DragValue::new(&mut self.image.width));
                                ui.add(egui::DragValue::new(&mut self.image.height));
                            });
                            ui.horizontal(|ui| {
                                ui.label("颜色");
                                ui.add(
                                    egui::DragValue::new(&mut self.image.color_r).range(0.0..=1.0),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.image.color_g).range(0.0..=1.0),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.image.color_b).range(0.0..=1.0),
                                );
                            });
                            if ui.button("移除 Image").clicked()
                                && let Err(error) = self.remove_component("Image")
                            {
                                self.status = error;
                            }
                        });
                    }
                }
                if has("Button") {
                    let button_header = if self.inspector_button_open {
                        "Button  v"
                    } else {
                        "Button  >"
                    };
                    if ui.button(button_header).clicked() {
                        self.inspector_button_open = !self.inspector_button_open;
                    }
                    if self.inspector_button_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("文字");
                                ui.text_edit_singleline(&mut self.button.label);
                            });
                            ui.horizontal(|ui| {
                                ui.label("位置/尺寸");
                                ui.add(egui::DragValue::new(&mut self.button.x));
                                ui.add(egui::DragValue::new(&mut self.button.y));
                                ui.add(egui::DragValue::new(&mut self.button.width));
                                ui.add(egui::DragValue::new(&mut self.button.height));
                            });
                            ui.horizontal(|ui| {
                                ui.label("背景");
                                ui.add(
                                    egui::DragValue::new(&mut self.button.bg_r).range(0.0..=1.0),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.button.bg_g).range(0.0..=1.0),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.button.bg_b).range(0.0..=1.0),
                                );
                                ui.label("文字色");
                                ui.add(
                                    egui::DragValue::new(&mut self.button.text_r).range(0.0..=1.0),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.button.text_g).range(0.0..=1.0),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.button.text_b).range(0.0..=1.0),
                                );
                            });
                            if ui.button("移除 Button").clicked()
                                && let Err(error) = self.remove_component("Button")
                            {
                                self.status = error;
                            }
                        });
                    }
                }
                if has("Sprite") {
                    ui.horizontal(|ui| {
                        ui.label("Sprite");
                        ui.text_edit_singleline(&mut self.sprite_reference);
                    });
                }
                if has("LuaScript") {
                    ui.horizontal(|ui| {
                        ui.label("Lua");
                        ui.text_edit_singleline(&mut self.lua_script);
                    });
                }
                if ui.button("应用修改").clicked() {
                    self.apply_inspector();
                }
                ui.separator();
                if ui.button("添加子实体").clicked() {
                    self.add_entity(Some(selected));
                }
                if ui.button("删除实体").clicked() {
                    self.delete_selected();
                    self.inspector.clear_selection();
                }
            });
    }

    fn apply_inspector(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(selected) = self.inspector.selected() else {
            self.status = "应用修改".to_owned();
            return;
        };
        let name = self.entity_name.clone();
        let transform = self.transform;
        let transform3d = self.transform3d;
        let mesh = self.mesh_renderer.clone();
        let camera3d = self.camera3d;
        let light = self.light;
        let collider3d = self.collider3d.clone();
        let text = self.text.clone();
        let image = self.image.clone();
        let button = self.button.clone();
        let sprite = self.sprite_reference.clone();
        let lua = self.lua_script.clone();
        // 3D 组件只在实体已有该组件时更新，避免给普通实体注入默认组件。
        let had_transform3d = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Transform3D")
        });
        let had_camera3d = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Camera3D")
        });
        let had_light = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Light")
        });
        let had_collider3d = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Collider3d")
        });
        let had_text = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Text")
        });
        let had_image = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Image")
        });
        let had_button = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Button")
        });
        let inspector = self.inspector.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "应用修改".to_owned();
            return;
        };
        let result =
            self.history
                .execute(scene, "Inspector", CommandKind::Property, move |scene| {
                    inspector.set_name(scene, name)?;
                    inspector.set_transform(scene, transform)?;
                    if had_transform3d {
                        inspector.set_transform3d(scene, transform3d)?;
                    }
                    if mesh.trim().is_empty() {
                        inspector.clear_mesh_renderer(scene)?;
                    } else {
                        inspector.set_mesh_renderer(scene, mesh)?;
                    }
                    if had_camera3d {
                        inspector.set_camera3d(scene, camera3d)?;
                    }
                    if had_light {
                        inspector.set_light(scene, light)?;
                    }
                    if had_collider3d {
                        inspector.set_collider3d(scene, collider3d)?;
                    }
                    if had_text {
                        inspector.set_text(scene, text)?;
                    }
                    if had_image {
                        inspector.set_image(scene, image)?;
                    }
                    if had_button {
                        inspector.set_button(scene, button)?;
                    }
                    if sprite.trim().is_empty() {
                        inspector.clear_sprite_reference(scene)?;
                    } else {
                        inspector.set_sprite_reference(scene, sprite)?;
                    }
                    if lua.trim().is_empty() {
                        inspector.clear_lua_script(scene)?;
                    } else {
                        inspector.set_lua_script(scene, lua)?;
                    }
                    Ok::<(), InspectorError>(())
                });
        self.status = match result {
            Ok(_) => format!("Inspector 已更新：EntityId={}", selected.0),
            Err(error) => error.to_string(),
        };
    }

    /// 为选中实体添加 Collider3d 组件（用当前字段编码，经 history 可撤销）。
    fn add_collider3d_component(&mut self) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        if self.inspector.selected().is_none() {
            return Ok(());
        }
        let inspector = self.inspector.clone();
        let collider3d = self.collider3d.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            "Add Collider3d",
            CommandKind::Property,
            move |scene| {
                inspector.set_collider3d(scene, collider3d)?;
                Ok::<(), InspectorError>(())
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// 移除选中实体的 Collider3d 组件（经 history 可撤销）。
    fn remove_collider3d_component(&mut self) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        if self.inspector.selected().is_none() {
            return Ok(());
        }
        let inspector = self.inspector.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            "Remove Collider3d",
            CommandKind::Property,
            move |scene| {
                inspector.clear_collider3d(scene)?;
                Ok::<(), InspectorError>(())
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// 选中实体的组件键列表（Inspector 组件管理用）。
    fn current_components(&self) -> Vec<String> {
        self.current_scene()
            .and_then(|s| {
                self.inspector.selected().map(|id| {
                    s.scene
                        .entities
                        .get(&id)
                        .map(|e| e.components.keys().cloned().collect())
                        .unwrap_or_default()
                })
            })
            .unwrap_or_default()
    }

    /// 为选中实体添加组件（默认值，经 history 可撤销）。
    fn add_component(&mut self, name: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        let Some(selected) = self.inspector.selected() else {
            return Ok(());
        };
        let Some(scene) = self.document.current_scene.as_ref() else {
            return Ok(());
        };
        if scene.scene.entities[&selected]
            .components
            .contains_key(name)
        {
            return Err(format!("实体已有 {name} 组件"));
        }
        let inspector = self.inspector.clone();
        let name = name.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            format!("Add {name}"),
            CommandKind::Property,
            move |scene| {
                match name.as_str() {
                    "Transform3D" => {
                        inspector.set_transform3d(scene, Transform3DFields::default())?
                    }
                    "Camera3D" => inspector.set_camera3d(scene, Camera3DFields::default())?,
                    "Light" => inspector.set_light(scene, LightFields::default())?,
                    "Collider3d" => inspector.set_collider3d(scene, Collider3dFields::default())?,
                    "MeshRenderer" => inspector.set_mesh_renderer(scene, "__ground_plane__")?,
                    "Text" => inspector.set_text(scene, TextFields::default())?,
                    "Image" => inspector.set_image(scene, ImageFields::default())?,
                    "Button" => inspector.set_button(scene, ButtonFields::default())?,
                    other => {
                        if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                            entity.components.insert(other.to_owned(), String::new());
                            scene.mark_dirty();
                        }
                    }
                }
                Ok::<(), InspectorError>(())
            },
        );
        match result {
            Ok(_) => {
                self.sync_inspector(selected);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    /// 移除选中实体的组件（经 history 可撤销）。
    fn remove_component(&mut self, name: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        let Some(selected) = self.inspector.selected() else {
            return Ok(());
        };
        let name = name.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            format!("Remove {name}"),
            CommandKind::Property,
            move |scene| {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.remove(&name);
                    scene.mark_dirty();
                }
                Ok::<(), InspectorError>(())
            },
        );
        match result {
            Ok(_) => {
                self.sync_inspector(selected);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn rename_selected(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let name = self.entity_name.trim().to_owned();
        let tree = self.tree.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result =
            self.history
                .execute(scene, "Rename entity", CommandKind::Rename, move |scene| {
                    let mut tree = tree;
                    tree.rename_selected(scene, name).map_err(|e| e.to_string())
                });
        match result {
            Ok(true) => {
                self.status = "Entity renamed".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }

    fn reparent_selected(&mut self, parent: Option<SceneEntityId>) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let tree = self.tree.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result = self.history.execute(
            scene,
            "Reparent entity",
            CommandKind::Reparent,
            move |scene| {
                let mut tree = tree;
                tree.reparent_selected(scene, parent)
                    .map_err(|e| e.to_string())
            },
        );
        match result {
            Ok(true) => {
                self.status = "Parent updated".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }

    fn delete_selected(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let tree = self.tree.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result =
            self.history
                .execute(scene, "Delete entity", CommandKind::Delete, move |scene| {
                    let mut tree = tree;
                    tree.delete_selected(scene).map_err(|e| e.to_string())
                });
        match result {
            Ok(true) => {
                if self.tree.selected().is_some_and(|id| {
                    self.document
                        .current_scene
                        .as_ref()
                        .is_none_or(|scene| !scene.scene.entities.contains_key(&id))
                }) {
                    self.tree.clear_selection();
                    self.inspector.clear_selection();
                }
                self.status = "Entity deleted".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }

    fn copy_selected_components(&mut self) {
        let Some(id) = self.tree.selected() else {
            return;
        };
        let Some(entity) = self
            .current_scene()
            .and_then(|scene| scene.scene.entities.get(&id))
        else {
            return;
        };
        self.inspector_clipboard = Some(ComponentClipboard {
            components: entity.components.clone(),
            resources: entity.resources.clone(),
            metadata: entity.metadata.clone(),
        });
        self.status = "Components copied".to_owned();
    }

    fn paste_selected_components(&mut self) {
        if self.is_playing() {
            return;
        }
        let Some(id) = self.tree.selected() else {
            return;
        };
        let Some(clipboard) = self.inspector_clipboard.clone() else {
            self.status = "Component clipboard is empty".to_owned();
            return;
        };
        if let Some(scene) = self.document.current_scene.as_mut() {
            let _ =
                self.history
                    .execute(scene, "Paste Components", CommandKind::Property, |scene| {
                        let entity = scene.scene.entities.get_mut(&id).ok_or(())?;
                        entity.components = clipboard.components.clone();
                        entity.resources = clipboard.resources.clone();
                        entity.metadata = clipboard.metadata.clone();
                        scene.mark_dirty();
                        Ok::<(), ()>(())
                    });
        }
        self.sync_inspector(id);
        self.status = "Components pasted".to_owned();
    }
}
