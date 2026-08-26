//! 行3 属性面板（Inspector）：实体层级/命名、组件管理、Transform/Camera/Light/Collider 编辑。

use crate::ComponentClipboard;
use crate::EditorApp;
use crate::util::strip_verbatim_prefix;
use editor::{
    ButtonFields, Camera3DFields, Collider3dFields, CommandKind, ImageFields, InspectorError,
    LightFields, TextFields, Transform3DFields, TransformFields,
};
use eframe::egui;
use engine::core::{
    prefab::Prefab,
    scene::{SceneEntity, SceneEntityId},
};

impl EditorApp {
    pub(crate) fn draw_inspector_panel(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(340.0);
        // 内容超出面板高度时可滚动。
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.heading("属性");
                if self
                    .current_scene()
                    .is_some_and(|scene| scene.is_prefab_document())
                {
                    ui.label("当前文档：Prefab 编辑模式");
                }
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
                if let Some((prefab_source, is_prefab_root)) =
                    self.current_scene().and_then(|scene| {
                        scene.scene.entities.get(&selected).and_then(|entity| {
                            entity.metadata.get("prefab.source").map(|source| {
                                (
                                    source.clone(),
                                    entity
                                        .metadata
                                        .get("prefab.root")
                                        .is_some_and(|value| value == "true"),
                                )
                            })
                        })
                    })
                {
                    ui.label(format!(
                        "Prefab 来源：{}{}",
                        prefab_source,
                        if is_prefab_root {
                            "（根实例）"
                        } else {
                            ""
                        }
                    ));
                    if is_prefab_root && ui.button("重新应用源 Prefab").clicked() {
                        self.reapply_selected_prefab();
                    }
                    if ui.button("打开源 Prefab").clicked() {
                        self.open_scene_resource(std::path::PathBuf::from(&prefab_source));
                    }
                    if is_prefab_root && ui.button("写回源并更新实例").clicked() {
                        self.save_selected_prefab_to_source();
                    }
                }
                if let Some(prefab_state) = self.prefab_override_summary(selected) {
                    match prefab_state {
                        Ok(prefab_overrides) => {
                            if prefab_overrides.is_empty() {
                                ui.label("Prefab Override：无");
                            } else {
                                ui.collapsing(
                                    format!("Prefab Override（{}）", prefab_overrides.len()),
                                    |ui| {
                                        for override_field in prefab_overrides {
                                            ui.label(override_field);
                                        }
                                    },
                                );
                            }
                        }
                        Err(error) => {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 140, 80),
                                format!("Prefab 诊断：{error}"),
                            );
                        }
                    }
                }
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
                                "Sprite",
                                "Tilemap",
                                "Animation",
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
                            draw_ui_layout_fields(
                                ui,
                                "text_ui_layout",
                                &mut self.text.space,
                                &mut self.text.anchor_x,
                                &mut self.text.anchor_y,
                            );
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
                            draw_ui_layout_fields(
                                ui,
                                "image_ui_layout",
                                &mut self.image.space,
                                &mut self.image.anchor_x,
                                &mut self.image.anchor_y,
                            );
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
                            draw_ui_layout_fields(
                                ui,
                                "button_ui_layout",
                                &mut self.button.space,
                                &mut self.button.anchor_x,
                                &mut self.button.anchor_y,
                            );
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
                    let sprite_header = if self.inspector_sprite_open {
                        "Sprite  v"
                    } else {
                        "Sprite  >"
                    };
                    if ui.button(sprite_header).clicked() {
                        self.inspector_sprite_open = !self.inspector_sprite_open;
                    }
                    if self.inspector_sprite_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("纹理");
                                ui.text_edit_singleline(&mut self.sprite_component.texture);
                            });
                            ui.horizontal(|ui| {
                                ui.label("尺寸");
                                ui.add(egui::DragValue::new(&mut self.sprite_component.size.x));
                                ui.add(egui::DragValue::new(&mut self.sprite_component.size.y));
                            });
                            ui.horizontal(|ui| {
                                ui.label("Pivot");
                                ui.add(
                                    egui::DragValue::new(&mut self.sprite_component.pivot.x)
                                        .range(0.0..=1.0),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.sprite_component.pivot.y)
                                        .range(0.0..=1.0),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("颜色");
                                for channel in &mut self.sprite_component.tint {
                                    ui.add(
                                        egui::DragValue::new(channel).range(0.0..=1.0).speed(0.01),
                                    );
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("排序层");
                                egui::ComboBox::from_id_salt("sprite_sorting_layer")
                                    .selected_text(&self.sprite_component.sorting.layer)
                                    .show_ui(ui, |ui| {
                                        for layer in [
                                            "background",
                                            "world",
                                            "default",
                                            "actor",
                                            "foreground",
                                            "ui",
                                        ] {
                                            ui.selectable_value(
                                                &mut self.sprite_component.sorting.layer,
                                                layer.to_owned(),
                                                layer,
                                            );
                                        }
                                    });
                                ui.label("Order");
                                ui.add(egui::DragValue::new(
                                    &mut self.sprite_component.sorting.order,
                                ));
                            });
                            if ui.button("移除 Sprite").clicked()
                                && let Err(error) = self.remove_component("Sprite")
                            {
                                self.status = error;
                            }
                            self.sprite_reference = self.sprite_component.to_component_str();
                        });
                    }
                }
                if has("Tilemap") {
                    let tilemap_header = if self.inspector_tilemap_open {
                        "Tilemap  v"
                    } else {
                        "Tilemap  >"
                    };
                    if ui.button(tilemap_header).clicked() {
                        self.inspector_tilemap_open = !self.inspector_tilemap_open;
                    }
                    if self.inspector_tilemap_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("纹理");
                                ui.text_edit_singleline(&mut self.tilemap.map.texture);
                            });
                            ui.horizontal(|ui| {
                                ui.label("Tile 尺寸");
                                ui.add(egui::DragValue::new(&mut self.tilemap.map.tile_size.x));
                                ui.add(egui::DragValue::new(&mut self.tilemap.map.tile_size.y));
                            });
                            ui.horizontal(|ui| {
                                ui.label("网格");
                                ui.add(
                                    egui::DragValue::new(&mut self.tilemap.map.width)
                                        .range(1..=256),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.tilemap.map.height)
                                        .range(1..=256),
                                );
                            });
                            let target_len = self.tilemap.map.width * self.tilemap.map.height;
                            if self.tilemap.map.tiles.len() != target_len {
                                self.tilemap.map.tiles.resize(target_len, 0);
                            }
                            if self.tilemap.map.solid.len() != target_len {
                                self.tilemap.map.solid.resize(target_len, false);
                            }
                            ui.horizontal(|ui| {
                                ui.label("画笔");
                                ui.add(egui::DragValue::new(&mut self.tilemap.brush).range(0..=6));
                                ui.checkbox(&mut self.tilemap.paint_collision, "画碰撞层");
                            });
                            ui.label("点击下方网格可直接编辑视觉层或碰撞层。");
                            let preview_width = self.tilemap.map.width.min(24);
                            let preview_height = self.tilemap.map.height.min(24);
                            egui::ScrollArea::both().max_height(260.0).show(ui, |ui| {
                                egui::Grid::new("tilemap_grid")
                                    .spacing(egui::vec2(2.0, 2.0))
                                    .show(ui, |ui| {
                                        for y in 0..preview_height {
                                            for x in 0..preview_width {
                                                let index = y * self.tilemap.map.width + x;
                                                let value = self.tilemap.map.tiles[index];
                                                let solid = self.tilemap.map.solid[index];
                                                let label = if self.tilemap.paint_collision {
                                                    if solid { "■" } else { "·" }
                                                } else if value == 0 {
                                                    "·"
                                                } else {
                                                    match value {
                                                        1 => "1",
                                                        2 => "2",
                                                        3 => "3",
                                                        4 => "4",
                                                        5 => "5",
                                                        _ => "6",
                                                    }
                                                };
                                                if ui
                                                    .add_sized(
                                                        [18.0, 18.0],
                                                        egui::Button::new(label),
                                                    )
                                                    .clicked()
                                                {
                                                    if self.tilemap.paint_collision {
                                                        self.tilemap.map.solid[index] =
                                                            !self.tilemap.map.solid[index];
                                                    } else {
                                                        self.tilemap.map.tiles[index] =
                                                            self.tilemap.brush.min(6);
                                                    }
                                                }
                                            }
                                            ui.end_row();
                                        }
                                    });
                            });
                            if ui.button("移除 Tilemap").clicked()
                                && let Err(error) = self.remove_component("Tilemap")
                            {
                                self.status = error;
                            }
                        });
                    }
                }
                if has("Animation") {
                    let animation_header = if self.inspector_animation_open {
                        "Animation  v"
                    } else {
                        "Animation  >"
                    };
                    if ui.button(animation_header).clicked() {
                        self.inspector_animation_open = !self.inspector_animation_open;
                    }
                    if self.inspector_animation_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("默认状态");
                                ui.text_edit_singleline(&mut self.animation.machine.default);
                            });
                            let mut remove_index = None;
                            for (index, clip) in self.animation.machine.clips.iter_mut().enumerate()
                            {
                                ui.separator();
                                ui.horizontal(|ui| {
                                    ui.label(format!("Clip {}", index + 1));
                                    if ui.small_button("删除").clicked() {
                                        remove_index = Some(index);
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("名称");
                                    ui.text_edit_singleline(&mut clip.name);
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Sheet");
                                    ui.text_edit_singleline(&mut clip.sheet);
                                });
                                ui.horizontal(|ui| {
                                    ui.label("列数/FPS");
                                    ui.add(egui::DragValue::new(&mut clip.cols).range(1..=128));
                                    ui.add(
                                        egui::DragValue::new(&mut clip.frame_rate)
                                            .speed(0.1)
                                            .range(0.1..=120.0),
                                    );
                                    ui.checkbox(&mut clip.looped, "循环");
                                });
                            }
                            if let Some(index) = remove_index {
                                self.animation.machine.clips.remove(index);
                                if self.animation.machine.clips.is_empty() {
                                    self.animation.machine.clips.push(
                                        engine::core::animation::AnimationClip {
                                            name: "idle".to_owned(),
                                            sheet: self.sprite_component.texture.clone(),
                                            cols: 1,
                                            frame_rate: 1.0,
                                            looped: true,
                                        },
                                    );
                                }
                                if !self
                                    .animation
                                    .machine
                                    .clips
                                    .iter()
                                    .any(|clip| clip.name == self.animation.machine.default)
                                {
                                    self.animation.machine.default =
                                        self.animation.machine.clips[0].name.clone();
                                }
                            }
                            if ui.button("添加 Clip").clicked() {
                                self.animation.machine.clips.push(
                                    engine::core::animation::AnimationClip {
                                        name: format!(
                                            "clip{}",
                                            self.animation.machine.clips.len() + 1
                                        ),
                                        sheet: self.sprite_component.texture.clone(),
                                        cols: 1,
                                        frame_rate: 8.0,
                                        looped: true,
                                    },
                                );
                            }
                            ui.label("动画数据通过 Inspector 编辑，运行时仍由 Lua 控制切换。");
                            if ui.button("移除 Animation").clicked()
                                && let Err(error) = self.remove_component("Animation")
                            {
                                self.status = error;
                            }
                        });
                    }
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
                    self.delete_selected_entity();
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
        let sprite_component = self.sprite_component.clone();
        let tilemap = self.tilemap.map.clone();
        let animation = self.animation.machine.clone();
        let sprite = self.sprite_component.to_component_str();
        let lua = self.lua_script.clone();
        let selected_sprite_ids: Vec<_> = self.scene_state.selected.iter().copied().collect();
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
        let had_tilemap = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Tilemap")
        });
        let had_animation = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Animation")
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
                    if sprite_component.texture.trim().is_empty() {
                        inspector.clear_sprite_reference(scene)?;
                    } else {
                        inspector.set_sprite_reference(scene, sprite.clone())?;
                        for id in &selected_sprite_ids {
                            if *id == selected {
                                continue;
                            }
                            if let Some(entity) = scene.scene.entities.get_mut(id)
                                && entity.components.contains_key("Sprite")
                            {
                                entity
                                    .components
                                    .insert("Sprite".to_owned(), sprite.clone());
                                scene.mark_dirty();
                            }
                        }
                    }
                    if had_tilemap && let Some(entity) = scene.scene.entities.get_mut(&selected) {
                        entity
                            .components
                            .insert("Tilemap".to_owned(), tilemap.to_component_str());
                        scene.mark_dirty();
                    }
                    if had_animation && let Some(entity) = scene.scene.entities.get_mut(&selected) {
                        entity
                            .components
                            .insert("Animation".to_owned(), animation.to_component_str());
                        scene.mark_dirty();
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
    pub(crate) fn add_component(&mut self, name: &str) -> Result<(), String> {
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
                    "Sprite" => inspector.set_sprite_reference(
                        scene,
                        engine::core::sprite::SpriteComponent {
                            texture: "assets/textures/player.png".to_owned(),
                            size: engine::core::transform::Vec2::new(128.0, 128.0),
                            pivot: engine::core::transform::Vec2::new(0.5, 0.5),
                            tint: [1.0; 4],
                            sorting: engine::core::sprite::SpriteSorting::default(),
                        }
                        .to_component_str(),
                    )?,
                    "Tilemap" => {
                        if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                            entity.components.insert(
                                "Tilemap".to_owned(),
                                engine::core::tilemap::Tilemap::new(
                                    engine::core::transform::Vec2::new(32.0, 32.0),
                                    8,
                                    8,
                                )
                                .map_err(|error| {
                                    InspectorError::InvalidAssetReference(error.to_string())
                                })?
                                .to_component_str(),
                            );
                            scene.mark_dirty();
                        }
                    }
                    "Animation" => {
                        if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                            entity.components.insert(
                                "Animation".to_owned(),
                                engine::core::animation::AnimationStateMachine::new(
                                    "idle",
                                    vec![engine::core::animation::AnimationClip {
                                        name: "idle".to_owned(),
                                        sheet: "assets/textures/player.png".to_owned(),
                                        cols: 1,
                                        frame_rate: 1.0,
                                        looped: true,
                                    }],
                                )
                                .map_err(|error| {
                                    InspectorError::InvalidAssetReference(error.to_string())
                                })?
                                .to_component_str(),
                            );
                            scene.mark_dirty();
                        }
                    }
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

    pub(crate) fn delete_selected_entity(&mut self) {
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
                    if matches!(self.delete_target, crate::DeleteTarget::Hierarchy) {
                        self.delete_target = crate::DeleteTarget::None;
                    }
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

    fn prefab_override_summary(
        &self,
        selected: SceneEntityId,
    ) -> Option<Result<Vec<String>, String>> {
        let scene = self.current_scene()?;
        let entity = scene.scene.entities.get(&selected)?;
        let prefab_source = entity.metadata.get("prefab.source")?;
        let source_id = entity
            .metadata
            .get("prefab.source_id")?
            .parse::<u64>()
            .ok()
            .map(SceneEntityId)?;
        let root = strip_verbatim_prefix(&self.project.root);
        let prefab = match Prefab::load(root.join(prefab_source)) {
            Ok(prefab) => prefab,
            Err(error) => return Some(Err(format!("无法加载源 Prefab：{error}"))),
        };
        let Some(source_entity) = prefab.entities.get(&source_id) else {
            return Some(Err(format!("源 Prefab 中缺少实体 {}", source_id.0)));
        };
        Some(Ok(diff_prefab_entity(source_entity, entity)))
    }

    fn reapply_selected_prefab(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(selected) = self.tree.selected() else {
            self.status = "请先选中 Prefab 根实例".to_owned();
            return;
        };
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&selected) else {
            self.status = "Prefab 根实例不存在".to_owned();
            return;
        };
        if entity
            .metadata
            .get("prefab.root")
            .is_none_or(|value| value != "true")
        {
            self.status = "只有 Prefab 根实例支持重新应用源".to_owned();
            return;
        }
        let Some(prefab_source) = entity.metadata.get("prefab.source").cloned() else {
            self.status = "当前实体没有 Prefab 来源".to_owned();
            return;
        };
        let root = strip_verbatim_prefix(&self.project.root);
        let prefab = match Prefab::load(root.join(&prefab_source)) {
            Ok(prefab) => prefab,
            Err(error) => {
                self.status = format!("重新加载 Prefab 失败：{error}");
                return;
            }
        };
        let rebuild =
            match collect_prefab_rebuild_state(&scene.scene, selected, &prefab_source, &prefab) {
                Ok(rebuild) => rebuild,
                Err(error) => {
                    self.status = error;
                    return;
                }
            };
        let new_root = std::rc::Rc::new(std::cell::Cell::new(None));
        let new_root_result = new_root.clone();
        let prefab_source_for_apply = prefab_source.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = self.history.execute(
            scene,
            "Reapply Prefab Source",
            CommandKind::Property,
            move |scene| {
                if let Some(parent_id) = rebuild.parent
                    && let Some(parent) = scene.scene.entities.get_mut(&parent_id)
                {
                    parent
                        .children
                        .retain(|child| *child != rebuild.instance_root);
                }
                for entity_id in &rebuild.instance_entities {
                    scene.scene.entities.remove(entity_id);
                }
                let instance = prefab
                    .instantiate(&mut scene.scene, rebuild.parent, &rebuild.overrides)
                    .map_err(|error| error.to_string())?;
                apply_prefab_instance_metadata(
                    &mut scene.scene,
                    &prefab_source_for_apply,
                    &instance,
                );
                migrate_prefab_editor_flags(
                    scene,
                    &rebuild.source_to_instance,
                    &instance.source_to_instance,
                );
                new_root.set(Some(instance.instance_root));
                scene.mark_dirty();
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(_) => {
                if let Some(new_root) = new_root_result.get() {
                    self.scene_state.selected.clear();
                    self.scene_state.selected.insert(new_root);
                    if let Some(scene) = self.document.current_scene.as_ref() {
                        let _ = self.tree.select(scene, new_root);
                    }
                    self.sync_hierarchy_metadata();
                    self.sync_inspector(new_root);
                }
                self.status = format!("已重新应用 Prefab：{prefab_source}");
            }
            Err(error) => self.status = format!("重新应用 Prefab 失败：{error}"),
        }
    }

    fn save_selected_prefab_to_source(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(selected) = self.tree.selected() else {
            self.status = "请先选中 Prefab 根实例".to_owned();
            return;
        };
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&selected) else {
            self.status = "Prefab 根实例不存在".to_owned();
            return;
        };
        if entity
            .metadata
            .get("prefab.root")
            .is_none_or(|value| value != "true")
        {
            self.status = "只有 Prefab 根实例支持写回源 Prefab".to_owned();
            return;
        }
        let Some(prefab_source) = entity.metadata.get("prefab.source").cloned() else {
            self.status = "当前实体没有 Prefab 来源".to_owned();
            return;
        };
        let root = strip_verbatim_prefix(&self.project.root);
        let prefab_path = root.join(&prefab_source);
        let existing_prefab = match Prefab::load(&prefab_path) {
            Ok(prefab) => prefab,
            Err(error) => {
                self.status = format!("加载源 Prefab 失败：{error}");
                return;
            }
        };
        let new_prefab =
            match build_prefab_from_instance_root(&scene.scene, selected, &existing_prefab) {
                Ok(prefab) => prefab,
                Err(error) => {
                    self.status = format!("生成源 Prefab 失败：{error}");
                    return;
                }
            };
        let roots = prefab_root_ids(&scene.scene, &prefab_source);
        let mut rebuilds = Vec::new();
        for root_id in roots {
            match collect_prefab_rebuild_state(&scene.scene, root_id, &prefab_source, &new_prefab) {
                Ok(rebuild) => rebuilds.push(rebuild),
                Err(error) => {
                    self.status = format!("实例 {} 无法更新：{error}", root_id.0);
                    return;
                }
            }
        }
        if let Err(error) = new_prefab.save(&prefab_path) {
            self.status = format!("写回源 Prefab 失败：{error}");
            return;
        }

        let new_selected_root = std::rc::Rc::new(std::cell::Cell::new(None));
        let selected_result = new_selected_root.clone();
        let prefab_source_for_apply = prefab_source.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = self.history.execute(
            scene,
            "Save Prefab Source",
            CommandKind::Property,
            move |scene| {
                for rebuild in &rebuilds {
                    if let Some(parent_id) = rebuild.parent
                        && let Some(parent) = scene.scene.entities.get_mut(&parent_id)
                    {
                        parent
                            .children
                            .retain(|child| *child != rebuild.instance_root);
                    }
                    for entity_id in &rebuild.instance_entities {
                        scene.scene.entities.remove(entity_id);
                    }
                    let instance = new_prefab
                        .instantiate(&mut scene.scene, rebuild.parent, &rebuild.overrides)
                        .map_err(|error| error.to_string())?;
                    apply_prefab_instance_metadata(
                        &mut scene.scene,
                        &prefab_source_for_apply,
                        &instance,
                    );
                    migrate_prefab_editor_flags(
                        scene,
                        &rebuild.source_to_instance,
                        &instance.source_to_instance,
                    );
                    if rebuild.instance_root == selected {
                        new_selected_root.set(Some(instance.instance_root));
                    }
                }
                scene.mark_dirty();
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(_) => {
                self.refresh_resources();
                if let Some(new_root) = selected_result.get() {
                    self.scene_state.selected.clear();
                    self.scene_state.selected.insert(new_root);
                    if let Some(scene) = self.document.current_scene.as_ref() {
                        let _ = self.tree.select(scene, new_root);
                    }
                    self.sync_hierarchy_metadata();
                    self.sync_inspector(new_root);
                }
                self.status = format!("已写回 Prefab 源并更新实例：{prefab_source}");
            }
            Err(error) => self.status = format!("更新 Prefab 实例失败：{error}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PrefabRebuildState {
    instance_root: SceneEntityId,
    parent: Option<SceneEntityId>,
    instance_entities: Vec<SceneEntityId>,
    source_to_instance: std::collections::BTreeMap<SceneEntityId, SceneEntityId>,
    overrides: std::collections::BTreeMap<(SceneEntityId, String), String>,
}

fn collect_prefab_rebuild_state(
    scene: &engine::core::scene::Scene,
    instance_root: SceneEntityId,
    prefab_source: &str,
    prefab: &Prefab,
) -> Result<PrefabRebuildState, String> {
    let root = scene
        .entities
        .get(&instance_root)
        .ok_or_else(|| format!("场景中不存在 Prefab 根实例 {}", instance_root.0))?;
    let instance_entities = collect_scene_subtree(scene, instance_root)?;
    let mut source_to_instance = std::collections::BTreeMap::new();
    for entity_id in &instance_entities {
        let entity = scene
            .entities
            .get(entity_id)
            .ok_or_else(|| format!("场景中不存在实体 {}", entity_id.0))?;
        if entity
            .metadata
            .get("prefab.source")
            .is_none_or(|value| value != prefab_source)
        {
            return Err("Prefab 实例子树包含非同源节点，暂不支持自动重建".to_owned());
        }
        let source_id = entity
            .metadata
            .get("prefab.source_id")
            .ok_or_else(|| format!("实体 {} 缺少 prefab.source_id", entity_id.0))?
            .parse::<u64>()
            .map(SceneEntityId)
            .map_err(|_| format!("实体 {} 的 prefab.source_id 非法", entity_id.0))?;
        if source_to_instance.insert(source_id, *entity_id).is_some() {
            return Err(format!("Prefab 实例中出现重复的源实体 {}", source_id.0));
        }
    }
    let prefab_ids = prefab
        .entities
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let instance_ids = source_to_instance
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if prefab_ids != instance_ids {
        return Err("Prefab 实例子树和源 Prefab 结构不一致，暂不支持自动重建".to_owned());
    }
    let overrides = collect_prefab_overrides(scene, prefab, &source_to_instance)?;
    Ok(PrefabRebuildState {
        instance_root,
        parent: root.parent,
        instance_entities,
        source_to_instance,
        overrides,
    })
}

fn build_prefab_from_instance_root(
    scene: &engine::core::scene::Scene,
    instance_root: SceneEntityId,
    existing_prefab: &Prefab,
) -> Result<Prefab, String> {
    let mut prefab = Prefab::from_scene(
        scene,
        instance_root,
        existing_prefab.asset_id,
        existing_prefab.name.clone(),
    )
    .map_err(|error| error.to_string())?;
    let mut source_ids = std::collections::BTreeMap::new();
    for entity_id in prefab.entities.keys().copied().collect::<Vec<_>>() {
        let scene_entity = scene
            .entities
            .get(&entity_id)
            .ok_or_else(|| format!("场景中不存在实体 {}", entity_id.0))?;
        let source_id = scene_entity
            .metadata
            .get("prefab.source_id")
            .ok_or_else(|| format!("实体 {} 缺少 prefab.source_id", entity_id.0))?
            .parse::<u64>()
            .map(SceneEntityId)
            .map_err(|_| format!("实体 {} 的 prefab.source_id 非法", entity_id.0))?;
        if source_ids.insert(entity_id, source_id).is_some() {
            return Err(format!("实例实体 {} 出现重复映射", entity_id.0));
        }
    }
    let root = *source_ids
        .get(&prefab.root)
        .ok_or_else(|| format!("实例根实体 {} 缺少 source_id 映射", prefab.root.0))?;
    let mut remapped = std::collections::BTreeMap::new();
    for (_, mut entity) in prefab.entities {
        let source_id = *source_ids
            .get(&entity.id)
            .ok_or_else(|| format!("实体 {} 缺少 source_id 映射", entity.id.0))?;
        entity.id = source_id;
        entity.parent = entity.parent.and_then(|id| source_ids.get(&id).copied());
        entity.children = entity
            .children
            .into_iter()
            .filter_map(|id| source_ids.get(&id).copied())
            .collect();
        if remapped.insert(source_id, entity).is_some() {
            return Err(format!("Prefab 写回时出现重复源实体 {}", source_id.0));
        }
    }
    prefab.root = root;
    prefab.entities = remapped;
    prefab.metadata = existing_prefab.metadata.clone();
    for entity in prefab.entities.values_mut() {
        entity.metadata.retain(|key, _| !key.starts_with("prefab."));
    }
    Ok(prefab)
}

fn prefab_root_ids(scene: &engine::core::scene::Scene, prefab_source: &str) -> Vec<SceneEntityId> {
    scene
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
                    .is_some_and(|value| value == prefab_source)
        })
        .map(|entity| entity.id)
        .collect()
}

fn collect_prefab_overrides(
    scene: &engine::core::scene::Scene,
    prefab: &Prefab,
    source_to_instance: &std::collections::BTreeMap<SceneEntityId, SceneEntityId>,
) -> Result<std::collections::BTreeMap<(SceneEntityId, String), String>, String> {
    let mut overrides = std::collections::BTreeMap::new();
    for (source_id, source_entity) in &prefab.entities {
        let instance_id = source_to_instance
            .get(source_id)
            .ok_or_else(|| format!("实例缺少源实体 {}", source_id.0))?;
        let instance_entity = scene
            .entities
            .get(instance_id)
            .ok_or_else(|| format!("场景中不存在实体 {}", instance_id.0))?;
        for field in diff_prefab_entity(source_entity, instance_entity) {
            let value = prefab_override_value(instance_entity, &field)?;
            overrides.insert((*source_id, field), value);
        }
    }
    Ok(overrides)
}

fn prefab_override_value(instance: &SceneEntity, field: &str) -> Result<String, String> {
    if field == "name" {
        Ok(instance.name.clone())
    } else if let Some(key) = field.strip_prefix("component:") {
        instance
            .components
            .get(key)
            .cloned()
            .ok_or_else(|| format!("实例缺少组件 {key}"))
    } else if let Some(key) = field.strip_prefix("metadata:") {
        instance
            .metadata
            .get(key)
            .cloned()
            .ok_or_else(|| format!("实例缺少元数据 {key}"))
    } else {
        Err(format!("不支持的 Prefab override 字段 {field}"))
    }
}

fn collect_scene_subtree(
    scene: &engine::core::scene::Scene,
    root: SceneEntityId,
) -> Result<Vec<SceneEntityId>, String> {
    fn visit(
        scene: &engine::core::scene::Scene,
        id: SceneEntityId,
        out: &mut Vec<SceneEntityId>,
        visiting: &mut std::collections::BTreeSet<SceneEntityId>,
    ) -> Result<(), String> {
        if !visiting.insert(id) {
            return Err(format!("实体 {} 的子树存在循环", id.0));
        }
        let entity = scene
            .entities
            .get(&id)
            .ok_or_else(|| format!("场景中不存在实体 {}", id.0))?;
        for child in &entity.children {
            visit(scene, *child, out, visiting)?;
        }
        out.push(id);
        Ok(())
    }

    let mut out = Vec::new();
    let mut visiting = std::collections::BTreeSet::new();
    visit(scene, root, &mut out, &mut visiting)?;
    Ok(out)
}

fn apply_prefab_instance_metadata(
    scene: &mut engine::core::scene::Scene,
    prefab_source: &str,
    instance: &engine::core::prefab::PrefabInstance,
) {
    for (source_id, entity_id) in &instance.source_to_instance {
        if let Some(entity) = scene.entities.get_mut(entity_id) {
            entity
                .metadata
                .insert("prefab.source".to_owned(), prefab_source.to_owned());
            entity
                .metadata
                .insert("prefab.source_id".to_owned(), source_id.0.to_string());
            entity.metadata.insert(
                "prefab.root".to_owned(),
                (entity.id == instance.instance_root).to_string(),
            );
        }
    }
}

fn migrate_prefab_editor_flags(
    scene: &mut editor::SceneDocument,
    old_map: &std::collections::BTreeMap<SceneEntityId, SceneEntityId>,
    new_map: &std::collections::BTreeMap<SceneEntityId, SceneEntityId>,
) {
    let mut hidden = std::collections::BTreeSet::new();
    let mut locked = std::collections::BTreeSet::new();
    for (source_id, instance_id) in old_map {
        let hidden_key = format!("editor.hidden.{}", instance_id.0);
        let locked_key = format!("editor.locked.{}", instance_id.0);
        if scene
            .scene
            .metadata
            .remove(&hidden_key)
            .is_some_and(|value| value == "true")
        {
            hidden.insert(*source_id);
        }
        if scene
            .scene
            .metadata
            .remove(&locked_key)
            .is_some_and(|value| value == "true")
        {
            locked.insert(*source_id);
        }
    }
    for source_id in hidden {
        if let Some(instance_id) = new_map.get(&source_id) {
            scene.scene.metadata.insert(
                format!("editor.hidden.{}", instance_id.0),
                "true".to_owned(),
            );
        }
    }
    for source_id in locked {
        if let Some(instance_id) = new_map.get(&source_id) {
            scene.scene.metadata.insert(
                format!("editor.locked.{}", instance_id.0),
                "true".to_owned(),
            );
        }
    }
}

fn diff_prefab_entity(source: &SceneEntity, instance: &SceneEntity) -> Vec<String> {
    let mut overrides = Vec::new();
    if source.name != instance.name {
        overrides.push("name".to_owned());
    }

    let component_keys = source
        .components
        .keys()
        .chain(instance.components.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    for key in component_keys {
        if source.components.get(&key) != instance.components.get(&key) {
            overrides.push(format!("component:{key}"));
        }
    }

    let metadata_keys = source
        .metadata
        .keys()
        .filter(|key| !key.starts_with("prefab."))
        .chain(
            instance
                .metadata
                .keys()
                .filter(|key| !key.starts_with("prefab.")),
        )
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    for key in metadata_keys {
        if source.metadata.get(&key) != instance.metadata.get(&key) {
            overrides.push(format!("metadata:{key}"));
        }
    }

    overrides
}

fn draw_ui_layout_fields(
    ui: &mut egui::Ui,
    id: &str,
    space: &mut String,
    anchor_x: &mut f32,
    anchor_y: &mut f32,
) {
    ui.horizontal(|ui| {
        ui.label("空间");
        egui::ComboBox::from_id_salt(format!("{id}_space"))
            .selected_text(if space == "canvas" {
                "Canvas"
            } else {
                "Screen"
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(space, "canvas".to_owned(), "Canvas");
                ui.selectable_value(space, "screen".to_owned(), "Screen");
            });
        ui.label("锚点");
        egui::ComboBox::from_id_salt(format!("{id}_anchor"))
            .selected_text(ui_anchor_preset_label(*anchor_x, *anchor_y))
            .show_ui(ui, |ui| {
                for (label, x, y) in [
                    ("左上", 0.0, 0.0),
                    ("上中", 0.5, 0.0),
                    ("右上", 1.0, 0.0),
                    ("左中", 0.0, 0.5),
                    ("中心", 0.5, 0.5),
                    ("右中", 1.0, 0.5),
                    ("左下", 0.0, 1.0),
                    ("下中", 0.5, 1.0),
                    ("右下", 1.0, 1.0),
                ] {
                    if ui
                        .selectable_label(
                            (*anchor_x - x).abs() < 0.001 && (*anchor_y - y).abs() < 0.001,
                            label,
                        )
                        .clicked()
                    {
                        *anchor_x = x;
                        *anchor_y = y;
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("Anchor XY");
        ui.add(egui::DragValue::new(anchor_x).range(0.0..=1.0).speed(0.05));
        ui.add(egui::DragValue::new(anchor_y).range(0.0..=1.0).speed(0.05));
    });
}

fn ui_anchor_preset_label(anchor_x: f32, anchor_y: f32) -> &'static str {
    const PRESETS: [(&str, f32, f32); 9] = [
        ("左上", 0.0, 0.0),
        ("上中", 0.5, 0.0),
        ("右上", 1.0, 0.0),
        ("左中", 0.0, 0.5),
        ("中心", 0.5, 0.5),
        ("右中", 1.0, 0.5),
        ("左下", 0.0, 1.0),
        ("下中", 0.5, 1.0),
        ("右下", 1.0, 1.0),
    ];
    PRESETS
        .into_iter()
        .find_map(|(label, x, y)| {
            ((anchor_x - x).abs() < 0.001 && (anchor_y - y).abs() < 0.001).then_some(label)
        })
        .unwrap_or("自定义")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorApp;
    use editor::open_project;
    use engine::core::{
        animation::AnimationStateMachine,
        logging::LogSink,
        prefab::Prefab,
        resource_db::AssetId,
        scene::Scene,
        scene::SceneEntityId,
        ui::{UiButton, UiLayoutSpace, UiText},
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_token() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos()
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rustlua-m14-01-inspector-{label}-{}",
            unique_token()
        ))
    }

    fn sample_project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../game_test_2d_production")
    }

    fn copy_dir(source: &Path, destination: &Path) {
        if !source.is_dir() {
            return;
        }
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let source_path = entry.path();
            let target_path = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                fs::create_dir_all(&target_path).unwrap();
                copy_dir(&source_path, &target_path);
            } else {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::copy(&source_path, &target_path).unwrap();
            }
        }
    }

    fn copy_sample_project(label: &str) -> PathBuf {
        let source = sample_project_root();
        assert!(
            source.join("project.toml").is_file(),
            "sample project missing: {}",
            source.display()
        );
        let root = temp_root(label);
        fs::create_dir_all(&root).unwrap();
        fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
        copy_dir(&source.join("scripts"), &root.join("scripts"));
        copy_dir(&source.join("scenes"), &root.join("scenes"));
        copy_dir(&source.join("assets"), &root.join("assets"));
        root
    }

    fn sample_app_with_production_scene(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_sample_project(label);
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/production.scene"));
        (app, root)
    }

    fn sample_app_with_menu_scene(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_sample_project(label);
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));
        (app, root)
    }

    fn select_entity(app: &mut EditorApp, id: SceneEntityId) {
        let scene = app.document.current_scene.as_ref().unwrap();
        app.tree.select(scene, id).unwrap();
        app.scene_state.selected.clear();
        app.scene_state.selected.insert(id);
        app.sync_inspector(id);
    }

    fn prefab_root(scene: &Scene, prefab_source: &str) -> SceneEntityId {
        scene
            .entities
            .values()
            .find(|entity| {
                entity
                    .metadata
                    .get("prefab.root")
                    .is_some_and(|value| value == "true")
                    && entity
                        .metadata
                        .get("prefab.source")
                        .is_some_and(|value| value == prefab_source)
            })
            .map(|entity| entity.id)
            .expect("sample scene should contain prefab root")
    }

    fn entity() -> SceneEntity {
        SceneEntity {
            id: SceneEntityId(1),
            name: "Enemy".to_owned(),
            parent: None,
            children: Vec::new(),
            components: std::collections::BTreeMap::from([
                ("Transform".to_owned(), "x=0;y=0".to_owned()),
                ("Sprite".to_owned(), "texture=enemy.png".to_owned()),
            ]),
            resources: std::collections::BTreeMap::new(),
            metadata: std::collections::BTreeMap::from([("team".to_owned(), "red".to_owned())]),
        }
    }

    #[test]
    fn diff_prefab_entity_reports_name_component_and_metadata_changes() {
        let source = entity();
        let mut instance = entity();
        instance.name = "Enemy Boss".to_owned();
        instance
            .components
            .insert("Transform".to_owned(), "x=10;y=4".to_owned());
        instance
            .metadata
            .insert("team".to_owned(), "blue".to_owned());
        instance.metadata.insert(
            "prefab.source".to_owned(),
            "assets/prefabs/enemy.prefab".to_owned(),
        );
        let overrides = diff_prefab_entity(&source, &instance);
        assert_eq!(
            overrides,
            vec![
                "name".to_owned(),
                "component:Transform".to_owned(),
                "metadata:team".to_owned()
            ]
        );
    }

    #[test]
    fn collect_prefab_rebuild_state_preserves_detected_overrides() {
        let mut source = Scene::new("prefab");
        source.add_entity(SceneEntityId(1), "Enemy", None).unwrap();
        source
            .add_entity(SceneEntityId(2), "HitBox", Some(SceneEntityId(1)))
            .unwrap();
        source
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Transform".to_owned(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".to_owned(),
            );
        let prefab = Prefab::from_scene(
            &source,
            SceneEntityId(1),
            AssetId::from_seed("enemy.prefab"),
            "Enemy",
        )
        .unwrap();

        let mut scene = Scene::new("runtime");
        let instance = prefab
            .instantiate(&mut scene, None, &std::collections::BTreeMap::new())
            .unwrap();
        apply_prefab_instance_metadata(&mut scene, "assets/prefabs/enemy.prefab", &instance);
        scene
            .entities
            .get_mut(&instance.instance_root)
            .unwrap()
            .name = "Boss".to_owned();
        let child = instance.source_to_instance[&SceneEntityId(2)];
        scene.entities.get_mut(&child).unwrap().components.insert(
            "Transform".to_owned(),
            "x=9;y=4;rotation=0;scale_x=1;scale_y=1".to_owned(),
        );

        let rebuild = collect_prefab_rebuild_state(
            &scene,
            instance.instance_root,
            "assets/prefabs/enemy.prefab",
            &prefab,
        )
        .unwrap();
        assert_eq!(
            rebuild
                .overrides
                .get(&(SceneEntityId(1), "name".to_owned())),
            Some(&"Boss".to_owned())
        );
        assert_eq!(
            rebuild
                .overrides
                .get(&(SceneEntityId(2), "component:Transform".to_owned())),
            Some(&"x=9;y=4;rotation=0;scale_x=1;scale_y=1".to_owned())
        );
    }

    #[test]
    fn collect_prefab_rebuild_state_rejects_mixed_subtree() {
        let mut source = Scene::new("prefab");
        source.add_entity(SceneEntityId(1), "Enemy", None).unwrap();
        let prefab = Prefab::from_scene(
            &source,
            SceneEntityId(1),
            AssetId::from_seed("enemy.prefab"),
            "Enemy",
        )
        .unwrap();

        let mut scene = Scene::new("runtime");
        let instance = prefab
            .instantiate(&mut scene, None, &std::collections::BTreeMap::new())
            .unwrap();
        apply_prefab_instance_metadata(&mut scene, "assets/prefabs/enemy.prefab", &instance);
        scene
            .add_entity(
                SceneEntityId(99),
                "LooseChild",
                Some(instance.instance_root),
            )
            .unwrap();

        let error = collect_prefab_rebuild_state(
            &scene,
            instance.instance_root,
            "assets/prefabs/enemy.prefab",
            &prefab,
        )
        .unwrap_err();
        assert!(error.contains("非同源节点") || error.contains("结构不一致"));
    }

    #[test]
    fn build_prefab_from_instance_root_strips_prefab_metadata() {
        let mut source = Scene::new("prefab");
        source.add_entity(SceneEntityId(1), "Enemy", None).unwrap();
        let prefab = Prefab::from_scene(
            &source,
            SceneEntityId(1),
            AssetId::from_seed("enemy.prefab"),
            "Enemy",
        )
        .unwrap();

        let mut instance_scene = Scene::new("runtime");
        let instance = prefab
            .instantiate(
                &mut instance_scene,
                None,
                &std::collections::BTreeMap::new(),
            )
            .unwrap();
        apply_prefab_instance_metadata(
            &mut instance_scene,
            "assets/prefabs/enemy.prefab",
            &instance,
        );
        instance_scene
            .entities
            .get_mut(&instance.instance_root)
            .unwrap()
            .metadata
            .insert("designer.note".to_owned(), "boss".to_owned());

        let rebuilt =
            build_prefab_from_instance_root(&instance_scene, instance.instance_root, &prefab)
                .unwrap();
        assert!(
            !rebuilt.entities[&rebuilt.root]
                .metadata
                .contains_key("prefab.source")
        );
        assert_eq!(
            rebuilt.entities[&rebuilt.root]
                .metadata
                .get("designer.note"),
            Some(&"boss".to_owned())
        );
    }

    #[test]
    fn sample_project_reapply_prefab_preserves_overrides() {
        let (mut app, root) = sample_app_with_production_scene("reapply");
        let target = {
            let scene = &app.document.current_scene.as_ref().unwrap().scene;
            prefab_root(scene, "assets/prefabs/slime_green.prefab")
        };
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            let entity = scene.scene.entities.get_mut(&target).unwrap();
            entity.name = "BossGreen".to_owned();
            entity.components.insert(
                "Transform".to_owned(),
                "x=77;y=19;rotation=0;scale_x=1;scale_y=1".to_owned(),
            );
            scene.mark_dirty();
        }
        select_entity(&mut app, target);

        app.reapply_selected_prefab();

        assert!(
            app.status.contains("已重新应用 Prefab"),
            "status should confirm prefab reapply"
        );
        let selected = app.tree.selected().unwrap();
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let entity = scene.entities.get(&selected).unwrap();
        assert_eq!(entity.name, "BossGreen");
        assert_eq!(
            entity.components.get("Transform").map(String::as_str),
            Some("x=77;y=19;rotation=0;scale_x=1;scale_y=1")
        );
        assert_eq!(
            entity.metadata.get("prefab.source").map(String::as_str),
            Some("assets/prefabs/slime_green.prefab")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_save_prefab_to_source_updates_all_instances() {
        let (mut app, root) = sample_app_with_production_scene("save-source");
        let (target, original_names) = {
            let scene = &app.document.current_scene.as_ref().unwrap().scene;
            let target = prefab_root(scene, "assets/prefabs/slime_green.prefab");
            let names = scene
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
                            .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
                })
                .map(|entity| entity.name.clone())
                .collect::<Vec<_>>();
            (target, names)
        };
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            let entity = scene.scene.entities.get_mut(&target).unwrap();
            entity.name = "SlimeGreenElite".to_owned();
            scene.mark_dirty();
        }
        select_entity(&mut app, target);

        app.save_selected_prefab_to_source();

        assert!(
            app.status.contains("已写回 Prefab 源并更新实例"),
            "status should confirm prefab save, got: {}",
            app.status
        );
        let saved = Prefab::load(root.join("assets/prefabs/slime_green.prefab")).unwrap();
        assert_eq!(saved.entities[&saved.root].name, "SlimeGreenElite");
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let names: Vec<_> = scene
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
                        .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
            })
            .map(|entity| entity.name.as_str())
            .collect();
        assert!(
            !names.is_empty(),
            "sample scene should still contain green slimes"
        );
        assert!(
            names.contains(&"SlimeGreenElite"),
            "selected instance should rebuild from updated source"
        );
        assert!(
            original_names
                .iter()
                .filter(|name| name.as_str() != "SlimeGreen1")
                .all(|name| names.contains(&name.as_str())),
            "other instance name overrides should be preserved"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_animation_edit_persists_after_save_and_reopen() {
        let (mut app, root) = sample_app_with_production_scene("animation-edit");
        select_entity(&mut app, SceneEntityId(2));
        app.animation.machine.default = "move".to_owned();
        let move_clip = app
            .animation
            .machine
            .clips
            .iter_mut()
            .find(|clip| clip.name == "move")
            .expect("player should have move clip");
        move_clip.frame_rate = 12.0;
        move_clip.looped = false;

        app.apply_inspector();
        app.save_scene();
        app.open_scene_resource(PathBuf::from("scenes/production.scene"));

        let reopened = app.document.current_scene.as_ref().unwrap();
        let animation = AnimationStateMachine::from_component_str(
            &reopened.scene.entities[&SceneEntityId(2)].components["Animation"],
        )
        .unwrap();
        assert_eq!(animation.default, "move");
        let move_clip = animation
            .clips
            .iter()
            .find(|clip| clip.name == "move")
            .expect("reopened animation should keep move clip");
        assert_eq!(move_clip.frame_rate, 12.0);
        assert!(!move_clip.looped);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_ui_adjustments_persist_after_save_and_reopen() {
        let (mut app, root) = sample_app_with_menu_scene("ui-adjust");
        select_entity(&mut app, SceneEntityId(3));
        app.text.text = "Production Sample Title".to_owned();
        app.text.space = "screen".to_owned();
        app.text.anchor_x = 1.0;
        app.text.anchor_y = 0.0;
        app.text.x = -48.0;
        app.text.y = 24.0;
        app.text.scale = 2.8;
        app.apply_inspector();

        select_entity(&mut app, SceneEntityId(5));
        app.button.label = "Begin Run".to_owned();
        app.button.space = "screen".to_owned();
        app.button.anchor_x = 1.0;
        app.button.anchor_y = 1.0;
        app.button.x = -260.0;
        app.button.y = -72.0;
        app.button.width = 260.0;
        app.button.height = 64.0;
        app.button.bg_r = 0.35;
        app.button.bg_g = 0.28;
        app.button.bg_b = 0.82;
        app.apply_inspector();

        app.save_scene();
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));
        let reopened = app.document.current_scene.as_ref().unwrap();
        let title = UiText::from_component_str(
            &reopened.scene.entities[&SceneEntityId(3)].components["Text"],
        )
        .unwrap();
        assert_eq!(title.text, "Production Sample Title");
        assert_eq!(title.space, UiLayoutSpace::Screen);
        assert_eq!(title.anchor, [1.0, 0.0]);
        assert_eq!(title.position, [-48.0, 24.0]);
        assert_eq!(title.scale, 2.8);

        let button = UiButton::from_component_str(
            &reopened.scene.entities[&SceneEntityId(5)].components["Button"],
        )
        .unwrap();
        assert_eq!(button.label, "Begin Run");
        assert_eq!(button.space, UiLayoutSpace::Screen);
        assert_eq!(button.anchor, [1.0, 1.0]);
        assert_eq!(button.position, [-260.0, -72.0]);
        assert_eq!(button.size, [260.0, 64.0]);
        assert_eq!(button.background, [0.35, 0.28, 0.82]);
        let _ = fs::remove_dir_all(root);
    }
}
