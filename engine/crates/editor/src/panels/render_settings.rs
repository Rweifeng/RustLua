//! 「渲染设置」独立弹出窗口：编辑当前工程 `project.toml` 的 `[render]` 段。
//!
//! 只放**生产画质项**（阴影 / 平面反射 / 水面 / 几何与植被），开发期 A/B 归因旗标
//! （`--no-cull`、`--skip-mesh` 等）不在这里。配置的唯一真值是 `project.toml` 的
//! `[render]` 段：本面板只负责读写它，并把结果应用到 Scene 视图与下一次 Play。
//!
//! 三项**构建期**参数（方向光阴影图边长 / 级联数 / 点光 cube 面边长）只能在渲染器创建时
//! 给定，因此保存后如变化会丢弃 Scene 渲染器、下一帧按新配置重建（Play 每次新建，天然生效）。

use crate::EditorApp;
use crate::util::strip_verbatim_prefix;
use crate::{SceneViewCache2d, SceneViewCache3d};
use eframe::egui;
use rustlua_render::render_config::{QualityPreset, RenderConfig};

/// 面板缓冲（挂在 `EditorApp` 上；每次打开时从 `project.toml` 重读）。
#[derive(Debug, Default)]
pub(crate) struct RenderSettingsUi {
    pub(crate) open: bool,
    /// 正在编辑的值。
    pub(crate) config: RenderConfig,
    /// 磁盘上的值（判定"有没有改动"，也是"需要重建"的基准）。
    pub(crate) saved: RenderConfig,
    pub(crate) message: String,
}

/// 平面反射分辨率档的中文名（值 = 除数；1 才是全分辨率）。
fn planar_label(divisor: u32) -> &'static str {
    match divisor {
        1 => "全分辨率",
        2 => "半分辨率",
        4 => "四分之一",
        8 => "八分之一",
        _ => "自定义",
    }
}

impl EditorApp {
    /// 菜单「文件 ▸ 渲染设置…」：从 `project.toml` 读取到缓冲并打开窗口。
    pub(crate) fn open_render_settings(&mut self) {
        let root = strip_verbatim_prefix(&self.project.root);
        let config = match RenderConfig::load_from(&root) {
            Ok(config) => {
                self.render_settings.message.clear();
                config.unwrap_or_default()
            }
            Err(error) => {
                self.render_settings.message = error;
                RenderConfig::default()
            }
        };
        self.render_settings.config = config.clone();
        self.render_settings.saved = config;
        self.render_settings.open = true;
    }

    /// 保存到 `project.toml` 并应用到当前渲染器。
    ///
    /// 构建期参数变化 ⇒ 丢弃 Scene 渲染器（下一帧按新配置重建）；其余项直接写进现有
    /// 渲染器。两条路径都要让视图缓存失效，否则画面会停在旧档位上。
    pub(crate) fn save_render_settings(&mut self) {
        let root = strip_verbatim_prefix(&self.project.root);
        let config = self.render_settings.config.clone().normalized();
        if config == self.render_settings.saved {
            self.render_settings.message = "渲染设置没有改动。".to_owned();
            return;
        }
        let rebuild = self.render_settings.saved.needs_renderer_rebuild(&config);
        if let Err(error) = config.save_to(&root) {
            self.render_settings.message = error;
            return;
        }
        if let Some(renderer) = self.scene_renderer.as_mut() {
            renderer.apply_render_config(&config);
        }
        // M88：`effect_overlay` 改了要重新挂（关掉也要真的撤掉）。
        self.apply_project_effect_overlay();
        if rebuild {
            self.scene_renderer = None;
            self.scene_renderer_error = None;
            self.scene_renderer_retry_at = None;
        }
        // 缓存键不含渲染档位：不主动作废，视图会命中旧帧。
        self.scene_view_cache_2d = SceneViewCache2d::default();
        self.scene_view_cache_3d = SceneViewCache3d::default();
        self.scene_screen_cache_key = None;
        self.render_settings.saved = config.clone();
        self.render_settings.config = config;
        self.render_settings.message = if rebuild {
            "已保存；阴影构建期参数已变，Scene 渲染器将按新配置重建（Play 下次生效）。".to_owned()
        } else {
            "已保存渲染设置（Scene 视图已应用，Play 下次生效）。".to_owned()
        };
    }

    /// 独立弹出「渲染设置」窗口（`update` 帧末绘制）。
    pub(crate) fn draw_render_settings_panel(&mut self, context: &egui::Context) {
        if !self.render_settings.open {
            return;
        }
        let mut open = self.render_settings.open;
        egui::Window::new("渲染设置")
            .open(&mut open)
            .resizable(true)
            .default_width(440.0)
            .show(context, |ui| {
                let root = strip_verbatim_prefix(&self.project.root);
                ui.label(format!(
                    "{}（[render] 段）",
                    root.join("project.toml").display()
                ));
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("render_settings_scroll")
                    .max_height(460.0)
                    .show(ui, |ui| {
                        self.draw_quality_preset(ui);
                        ui.separator();
                        self.draw_shadow_group(ui);
                        ui.separator();
                        self.draw_reflection_group(ui);
                        ui.separator();
                        self.draw_geometry_group(ui);
                        self.draw_effect_group(ui);
                    });
                ui.separator();
                if self.render_settings.config != self.render_settings.saved {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 200, 90),
                        "有未保存的改动（保存后写入 project.toml）。",
                    );
                }
                if !self.render_settings.message.is_empty() {
                    ui.label(&self.render_settings.message);
                }
                ui.horizontal(|ui| {
                    if ui.button("保存").clicked() {
                        self.save_render_settings();
                    }
                    if ui.button("取消").clicked() {
                        self.render_settings.open = false;
                    }
                    if ui.button("恢复默认（不落盘）").clicked() {
                        self.render_settings.config = RenderConfig::default();
                        self.render_settings.message.clear();
                    }
                });
            });
        self.render_settings.open = open;
    }

    /// 画质预设：只是"一键填值"，文件里存的仍是逐项字段。
    fn draw_quality_preset(&mut self, ui: &mut egui::Ui) {
        ui.label("画质预设");
        ui.horizontal(|ui| {
            for (preset, label) in [
                (QualityPreset::Default, "默认"),
                (QualityPreset::Medium, "中"),
                (QualityPreset::Low, "低"),
            ] {
                if ui.button(label).clicked() {
                    self.render_settings.config =
                        self.render_settings.config.clone().with_quality(preset);
                }
            }
        });
        ui.weak("预设只改平面反射分辨率 / 水面廉价光照 / 点光面边长；其余字段保持原值。");
    }

    fn draw_shadow_group(&mut self, ui: &mut egui::Ui) {
        let config = &mut self.render_settings.config;
        ui.label("阴影");
        egui::Grid::new("render_settings_shadow")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("方向光阴影图边长");
                egui::ComboBox::from_id_salt("render_shadow_resolution")
                    .selected_text(format!("{}（保存后重建）", config.shadow_resolution))
                    .show_ui(ui, |ui| {
                        for value in [512, 1024, 2048, 4096] {
                            ui.selectable_value(
                                &mut config.shadow_resolution,
                                value,
                                format!("{value}（保存后重建）"),
                            );
                        }
                    });
                ui.end_row();

                ui.label("级联层数");
                egui::ComboBox::from_id_salt("render_shadow_cascades")
                    .selected_text(format!("{}（保存后重建）", config.shadow_cascades))
                    .show_ui(ui, |ui| {
                        for value in 1..=4 {
                            ui.selectable_value(
                                &mut config.shadow_cascades,
                                value,
                                format!("{value}（保存后重建）"),
                            );
                        }
                    });
                ui.end_row();

                ui.label("点光阴影面边长");
                egui::ComboBox::from_id_salt("render_point_shadow_resolution")
                    .selected_text(format!("{}（保存后重建）", config.point_shadow_resolution))
                    .show_ui(ui, |ui| {
                        for value in [512, 1024] {
                            ui.selectable_value(
                                &mut config.point_shadow_resolution,
                                value,
                                format!("{value}（保存后重建）"),
                            );
                        }
                    });
                ui.end_row();

                ui.label("点光阴影盏数");
                ui.add(egui::DragValue::new(&mut config.point_shadow_lights).range(1..=8));
                ui.end_row();

                ui.label("阴影投射距离（米，0=关）");
                ui.add(
                    egui::DragValue::new(&mut config.shadow_caster_distance)
                        .range(0.0..=500.0)
                        .speed(1.0)
                        .max_decimals(1),
                );
                ui.end_row();
            });
    }

    fn draw_reflection_group(&mut self, ui: &mut egui::Ui) {
        let config = &mut self.render_settings.config;
        ui.label("反射与水面");
        egui::Grid::new("render_settings_reflection")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("平面反射分辨率");
                egui::ComboBox::from_id_salt("render_planar_divisor")
                    .selected_text(planar_label(config.planar_divisor))
                    .show_ui(ui, |ui| {
                        for value in [1, 2, 4, 8] {
                            ui.selectable_value(
                                &mut config.planar_divisor,
                                value,
                                planar_label(value),
                            );
                        }
                    });
                ui.end_row();

                ui.label("水面廉价光照");
                ui.checkbox(&mut config.water_cheap, "跳过 SH9 / 点光 / 级联阴影");
                ui.end_row();

                ui.label("水面折射增益");
                ui.add(
                    egui::DragValue::new(&mut config.water_refraction)
                        .range(0.0..=0.01)
                        .speed(0.0001)
                        .max_decimals(5),
                )
                .on_hover_text("0 = 旧的固定偏移；真实量级约 0.0008");
                ui.end_row();

                ui.label("水下视角强度");
                ui.add(
                    egui::DragValue::new(&mut config.underwater_view)
                        .range(0.0..=1.0)
                        .speed(0.01)
                        .max_decimals(2),
                )
                .on_hover_text("0 = 关闭；1 = 界面像素完全换成 Snell 窗口 / 全反射镜像");
                ui.end_row();

                ui.label("折射步进次数");
                ui.add(egui::DragValue::new(&mut config.water_refract_steps).range(0..=32));
                ui.end_row();
            });
    }

    fn draw_geometry_group(&mut self, ui: &mut egui::Ui) {
        let config = &mut self.render_settings.config;
        ui.label("几何与植被");
        egui::Grid::new("render_settings_geometry")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("最大绘制距离（米，0=关）");
                ui.add(
                    egui::DragValue::new(&mut config.draw_distance)
                        .range(0.0..=1000.0)
                        .speed(1.0)
                        .max_decimals(1),
                );
                ui.end_row();

                ui.label("距离档只作用于");
                let mut filter = config.draw_distance_filter.clone().unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut filter)
                            .hint_text("网格路径子串，留空 = 全体（只掐草用 grass）"),
                    )
                    .changed()
                {
                    config.draw_distance_filter =
                        (!filter.trim().is_empty()).then(|| filter.trim().to_owned());
                }
                ui.end_row();

                ui.label("地形廉价细节");
                ui.checkbox(&mut config.terrain_cheap, "跳过近处视差 / 裂缝 AO / 微阴影");
                ui.end_row();

                ui.label("普通网格廉价着色");
                ui.checkbox(&mut config.crowd_cheap, "全价 PBR → 廉价档");
                ui.end_row();

                ui.label("剖切封口深度（米，0=关）");
                ui.add(
                    egui::DragValue::new(&mut config.clip_cap)
                        .range(0.0..=128.0)
                        .speed(0.1)
                        .max_decimals(2),
                );
                ui.end_row();

                ui.label("封口吃到单面网格");
                ui.checkbox(&mut config.clip_cap_both_sides, "glb 壳也画实体剖面");
                ui.end_row();
            });
    }
    /// M98：粒子软边。放在这里而不是 Shader Graph 面板：它改的是**粒子绘制管线**的
    /// 全局宽度（`[render] particle_softness`），与叠加层模板无关。
    fn draw_effect_group(&mut self, ui: &mut egui::Ui) {
        let config = &mut self.render_settings.config;
        ui.label("特效与粒子");
        egui::Grid::new("render_settings_effect")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("粒子软边宽度（0=硬边）");
                ui.add(
                    egui::DragValue::new(&mut config.particle_softness)
                        .range(0.0..=0.5)
                        .speed(0.005)
                        .max_decimals(3),
                )
                .on_hover_text(
                    "GPU 粒子与场景深度交界处的淡出宽度；越大软边越宽，0 = 硬切边。默认为 0.02",
                );
                ui.end_row();
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorApp;
    use rustlua_infra::logging::LogSink;

    fn temp_project(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rustlua-editor-render-{tag}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app_for(root: &std::path::Path) -> EditorApp {
        EditorApp::new(
            editor::ProjectInfo {
                root: root.to_path_buf(),
                version: "test".into(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        )
    }

    #[test]
    fn opens_defaults_when_project_has_no_render_section() {
        let root = temp_project("defaults");
        std::fs::write(root.join("project.toml"), "[project]\nname = \"demo\"\n").unwrap();
        let mut app = app_for(&root);
        app.open_render_settings();
        assert_eq!(app.render_settings.config, RenderConfig::default());
        assert_eq!(app.render_settings.config, app.render_settings.saved);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_writes_section_and_round_trips() {
        let root = temp_project("save");
        std::fs::write(
            root.join("project.toml"),
            "[project]\nname = \"demo\"\n\n[audio]\nmaster = 0.5\n",
        )
        .unwrap();
        let mut app = app_for(&root);
        app.open_render_settings();
        app.render_settings.config.planar_divisor = 4;
        app.render_settings.config.water_cheap = true;
        app.render_settings.config.draw_distance_filter = Some("grass".to_owned());
        app.save_render_settings();

        let text = std::fs::read_to_string(root.join("project.toml")).unwrap();
        assert!(text.contains("[render]"), "{text}");
        assert!(text.contains("planar_divisor = 4"), "{text}");
        assert!(text.contains("water_cheap = true"), "{text}");
        assert!(text.contains("draw_distance_filter = \"grass\""), "{text}");
        assert!(
            text.contains("[audio]\nmaster = 0.5"),
            "其它段原样保留：{text}"
        );

        // 重新打开 = 磁盘上的新值，且再保存不会重复写段。
        app.open_render_settings();
        assert_eq!(app.render_settings.config.planar_divisor, 4);
        let before = text.clone();
        app.save_render_settings();
        assert_eq!(
            std::fs::read_to_string(root.join("project.toml")).unwrap(),
            before,
            "无改动保存不该重写文件"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn construction_time_changes_are_detected_but_runtime_ones_are_not() {
        let base = RenderConfig::default();
        assert!(!base.needs_renderer_rebuild(&RenderConfig {
            planar_divisor: 8,
            ..RenderConfig::default()
        }));
        assert!(base.needs_renderer_rebuild(&RenderConfig {
            shadow_cascades: 2,
            ..RenderConfig::default()
        }));
    }

    #[test]
    fn preset_buttons_fill_only_the_preset_fields() {
        let mut config = RenderConfig {
            clip_cap: 2.0,
            particle_softness: 0.3,
            ..RenderConfig::default()
        };
        config = config.with_quality(QualityPreset::Low);
        assert_eq!(config.planar_divisor, 4);
        assert!(config.water_cheap);
        assert_eq!(config.point_shadow_resolution, 512);
        assert_eq!(config.clip_cap, 2.0, "预设不碰未包含字段");
        assert_eq!(config.particle_softness, 0.3, "预设不碰粒子软边宽度");
    }

    /// M98：面板能无头画出「特效与粒子」组，且粒子软边宽度走同一条保存/套用路径。
    #[test]
    fn particle_softness_slider_is_drawn_and_saved() {
        let root = temp_project("softness");
        std::fs::write(root.join("project.toml"), "[project]\nname = \"demo\"\n").unwrap();
        let mut app = app_for(&root);
        app.open_render_settings();
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            app.draw_render_settings_panel(context);
        });

        app.render_settings.config.particle_softness = 0.25;
        app.save_render_settings();
        let text = std::fs::read_to_string(root.join("project.toml")).unwrap();
        assert!(text.contains("particle_softness = 0.25"), "{text}");

        // 重新打开 = 磁盘值；默认档仍是 0.02。
        app.open_render_settings();
        assert!((app.render_settings.config.particle_softness - 0.25).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(&root);
    }
}
