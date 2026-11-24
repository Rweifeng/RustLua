//! 程序化素材面板（M82 接入编辑器）：参数可改、结果可预览。
//!
//! 四个页签，各自「改参数 → 即时重生成 → 预览」：
//! * **贴图**：`texture_gen::TextureSpec`（棋盘/噪声/渐变/条纹）→ RGBA → egui 图像；
//! * **文字**：`text_texture::render_text`（工程/编辑器字体）→ RGBA → egui 图像；
//! * **合成音效**：`synth::render_samples`（波形/噪声 + ADSR + biquad 滤波）→ 波形与 RMS/峰值读数；
//! * **立体**：参数化 primitive + 生成材质 → **离屏 3D 渲染**（真渲染器）→ 回读成 egui 图像。
//!
//! 贴图/文字/音效是**纯 CPU**（不占渲染器）；「立体」页用宿主 `gpu` 建一台专属离屏渲染器，
//! 把 `__primitive__` 网格 + `__texture__?` 生成材质真画一帧再回读（与 `texture_material3d` 同一条渲染路径）。
//! 每页只在参数**变化**时重算（判等上次 spec），拖滑杆不逐帧重建。
//!
//! 入口：工具栏「工具 ▸ 程序化素材…」把 `show` 置真（`egui::Window` 由它控制显隐）。

use eframe::egui;
use engine::host::{BoundsSource, Scene3d, Scene3dBounds, Scene3dDrawable, SceneMesh};
use rustlua_audio::synth::{
    FilterKind, FilterSpec, SYNTH_SAMPLE_RATE, SynthSpec, SynthWave, peak, render_samples, rms,
};
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, FrameViews, RenderView, Viewport};
use rustlua_gpu::Renderer;
use rustlua_math::math3d::{Aabb3, Mat4, Vec3};
use rustlua_render::light::Light;
use rustlua_render::primitives::PrimitiveSpec;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};
use rustlua_render::text_texture::{FontArc, TextSpec, load_font, render_text};
use rustlua_render::texture_gen::{TextureKind, TextureSpec};

use crate::EditorApp;

/// 立体预览的离屏尺寸（固定，不需要 resize 逻辑）。
const PREVIEW_W: u32 = 320;
const PREVIEW_H: u32 = 240;
/// 立体预览里材质贴图的哨兵键（每次渲染登记进渲染器）。
const SOLID_MATERIAL_KEY: &str = "__texture__?editor-solid";

const TEXTURE_KINDS: [TextureKind; 4] = [
    TextureKind::Checker,
    TextureKind::Noise,
    TextureKind::Gradient,
    TextureKind::Stripes,
];
const WAVES: [SynthWave; 5] = [
    SynthWave::Sine,
    SynthWave::Square,
    SynthWave::Saw,
    SynthWave::Triangle,
    SynthWave::Noise,
];
const SOLID_KINDS: [SolidKind; 6] = [
    SolidKind::Box,
    SolidKind::Sphere,
    SolidKind::Cylinder,
    SolidKind::Cone,
    SolidKind::Torus,
    SolidKind::Plane,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Texture,
    Text,
    Synth,
    Solid,
}

/// 「立体」页可选的基本体（映射到 `primitives::PrimitiveSpec`）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum SolidKind {
    Box,
    Sphere,
    Cylinder,
    Cone,
    Torus,
    Plane,
}

impl SolidKind {
    const fn token(self) -> &'static str {
        match self {
            Self::Box => "立方体",
            Self::Sphere => "球",
            Self::Cylinder => "圆柱",
            Self::Cone => "圆锥",
            Self::Torus => "环面",
            Self::Plane => "平面",
        }
    }

    /// 用单一 `size` 生成对应规格（圆类取半径 = size/2）。
    fn build(self, size: f32) -> PrimitiveSpec {
        let size = size.max(0.05);
        let half = (size * 0.5).max(0.05);
        let ring = (half * 0.33).max(0.05);
        match self {
            Self::Box => PrimitiveSpec::Box {
                w: size,
                h: size,
                d: size,
            },
            Self::Sphere => PrimitiveSpec::Sphere { r: half, seg: 24 },
            Self::Cylinder => PrimitiveSpec::Cylinder {
                r: half,
                h: size,
                seg: 24,
            },
            Self::Cone => PrimitiveSpec::Cone {
                r: half,
                h: size,
                seg: 24,
            },
            Self::Torus => PrimitiveSpec::Torus {
                big_r: half,
                r: ring,
                seg: 24,
                ring_seg: 12,
            },
            Self::Plane => PrimitiveSpec::Plane { w: size, h: size },
        }
    }
}

/// 面板状态（挂在 `EditorApp` 上）。
pub(crate) struct ProceduralState {
    /// 窗口是否打开（绑到 `egui::Window::open`）。
    pub show: bool,
    tab: Tab,
    // ---- 贴图 ----
    tex_kind: TextureKind,
    tex_size: u32,
    tex_scale: f32,
    tex_seed: u32,
    tex_colors: [[u8; 4]; 2],
    tex_tex: Option<egui::TextureHandle>,
    tex_last: Option<TextureSpec>,
    tex_info: String,
    // ---- 文字 ----
    text_input: String,
    text_size: u32,
    text_color: [u8; 4],
    text_bg: [u8; 4],
    text_tex: Option<egui::TextureHandle>,
    text_last: Option<(String, u32, [u8; 4], [u8; 4])>,
    text_info: String,
    font: Option<FontArc>,
    font_tried: bool,
    // ---- 合成音效 ----
    synth: SynthSpec,
    synth_use_filter: bool,
    synth_samples: Vec<f32>,
    synth_last: Option<SynthSpec>,
    synth_info: String,
    // ---- 立体 ----
    solid_kind: SolidKind,
    solid_size: f32,
    mat_colors: [[u8; 4]; 2],
    solid_last: Option<(SolidKind, u32, [[u8; 4]; 2])>,
    solid_info: String,
    preview_renderer: Option<Renderer>,
    preview_target: Option<wgpu::Texture>,
    preview_tex: Option<egui::TextureHandle>,
}

impl Default for ProceduralState {
    fn default() -> Self {
        Self {
            show: false,
            tab: Tab::Texture,
            tex_kind: TextureKind::Checker,
            tex_size: 64,
            tex_scale: 4.0,
            tex_seed: 0,
            tex_colors: [[235, 90, 90, 255], [80, 120, 235, 255]],
            tex_tex: None,
            tex_last: None,
            tex_info: String::new(),
            text_input: "B2-42".to_owned(),
            text_size: 32,
            text_color: [255, 240, 200, 255],
            text_bg: [24, 28, 36, 255],
            text_tex: None,
            text_last: None,
            text_info: String::new(),
            font: None,
            font_tried: false,
            synth: SynthSpec::default(),
            synth_use_filter: false,
            synth_samples: Vec::new(),
            synth_last: None,
            synth_info: String::new(),
            solid_kind: SolidKind::Box,
            solid_size: 1.2,
            mat_colors: [[240, 180, 90, 255], [40, 70, 130, 255]],
            solid_last: None,
            solid_info: String::new(),
            preview_renderer: None,
            preview_target: None,
            preview_tex: None,
        }
    }
}

impl EditorApp {
    /// 画「程序化素材」窗（未打开即早退）。入口在工具栏「工具」菜单里把 `show` 置真。
    pub(crate) fn draw_procedural_panel(&mut self, context: &egui::Context) {
        if !self.procedural.show {
            return;
        }
        let mut open = true;
        egui::Window::new("程序化素材")
            .open(&mut open)
            .default_width(560.0)
            .default_height(540.0)
            .resizable(true)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.procedural.tab, Tab::Texture, "贴图");
                    ui.selectable_value(&mut self.procedural.tab, Tab::Text, "文字");
                    ui.selectable_value(&mut self.procedural.tab, Tab::Synth, "合成音效");
                    ui.selectable_value(&mut self.procedural.tab, Tab::Solid, "立体");
                });
                ui.separator();
                match self.procedural.tab {
                    Tab::Texture => self.draw_procedural_texture(ui, context),
                    Tab::Text => self.draw_procedural_text(ui, context),
                    Tab::Synth => self.draw_procedural_synth(ui),
                    Tab::Solid => self.draw_procedural_solid(ui, context),
                }
            });
        self.procedural.show = open && self.procedural.show;
    }

    fn draw_procedural_texture(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        egui::ComboBox::from_label("种类")
            .selected_text(self.procedural.tex_kind.token())
            .show_ui(ui, |ui| {
                for kind in TEXTURE_KINDS {
                    ui.selectable_value(&mut self.procedural.tex_kind, kind, kind.token());
                }
            });
        ui.add(
            egui::Slider::new(&mut self.procedural.tex_size, 4..=1024)
                .text("边长")
                .logarithmic(true),
        );
        ui.add(egui::Slider::new(&mut self.procedural.tex_scale, 1.0..=64.0).text("频率"));
        if self.procedural.tex_kind == TextureKind::Noise {
            ui.add(egui::DragValue::new(&mut self.procedural.tex_seed).prefix("seed "));
        }
        ui.horizontal(|ui| {
            color_edit(ui, "色 A", &mut self.procedural.tex_colors[0]);
            color_edit(ui, "色 B", &mut self.procedural.tex_colors[1]);
        });

        let mut spec = TextureSpec::default_for(self.procedural.tex_kind);
        spec.size = self.procedural.tex_size;
        spec.scale = self.procedural.tex_scale;
        spec.seed = self.procedural.tex_seed;
        spec.colors = self.procedural.tex_colors.to_vec();
        if self.procedural.tex_last.as_ref() != Some(&spec) {
            match spec.generate() {
                Ok(asset) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [asset.width() as usize, asset.height() as usize],
                        asset.pixels(),
                    );
                    self.procedural.tex_tex = Some(context.load_texture(
                        "procedural_texture",
                        image,
                        egui::TextureOptions::NEAREST,
                    ));
                    self.procedural.tex_info = format!(
                        "{}×{} · {}",
                        asset.width(),
                        asset.height(),
                        spec.kind.token()
                    );
                }
                Err(error) => {
                    self.procedural.tex_tex = None;
                    self.procedural.tex_info = format!("生成失败：{error}");
                }
            }
            self.procedural.tex_last = Some(spec);
        }

        ui.label(&self.procedural.tex_info);
        ui.separator();
        if let Some(texture) = &self.procedural.tex_tex {
            let max = ui.available_size();
            ui.add(egui::Image::new(texture).max_size(egui::vec2(max.x, max.y * 0.86)));
        }
    }

    fn draw_procedural_text(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label("文本");
            ui.text_edit_singleline(&mut self.procedural.text_input);
        });
        ui.add(egui::Slider::new(&mut self.procedural.text_size, 8..=200).text("字号"));
        ui.horizontal(|ui| {
            color_edit(ui, "文字色", &mut self.procedural.text_color);
            color_edit(ui, "背景色", &mut self.procedural.text_bg);
        });

        if !self.procedural.font_tried {
            self.procedural.font_tried = true;
            self.procedural.font = load_editor_font();
        }

        let signature = (
            self.procedural.text_input.clone(),
            self.procedural.text_size,
            self.procedural.text_color,
            self.procedural.text_bg,
        );
        if self.procedural.text_last.as_ref() != Some(&signature) {
            if let Some(font) = self.procedural.font.as_ref() {
                let spec = TextSpec {
                    font_px: self.procedural.text_size,
                    padding: 4,
                    color: self.procedural.text_color,
                    background: self.procedural.text_bg,
                    max_width: None,
                };
                match render_text(font, &self.procedural.text_input, &spec) {
                    Ok(asset) => {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [asset.width() as usize, asset.height() as usize],
                            asset.pixels(),
                        );
                        self.procedural.text_tex = Some(context.load_texture(
                            "procedural_text",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                        self.procedural.text_info = format!("{}×{}", asset.width(), asset.height());
                    }
                    Err(error) => {
                        self.procedural.text_tex = None;
                        self.procedural.text_info = format!("栅格化失败：{error}");
                    }
                }
            } else {
                self.procedural.text_tex = None;
                self.procedural.text_info =
                    "编辑器字体缺失（engine/crates/editor/fonts/）".to_owned();
            }
            self.procedural.text_last = Some(signature);
        }

        ui.label(&self.procedural.text_info);
        ui.separator();
        if let Some(texture) = &self.procedural.text_tex {
            let max = ui.available_size();
            ui.add(egui::Image::new(texture).max_size(egui::vec2(max.x, max.y * 0.86)));
        }
    }

    fn draw_procedural_synth(&mut self, ui: &mut egui::Ui) {
        egui::ComboBox::from_label("波形")
            .selected_text(self.procedural.synth.wave.token())
            .show_ui(ui, |ui| {
                for wave in WAVES {
                    ui.selectable_value(&mut self.procedural.synth.wave, wave, wave.token());
                }
            });
        ui.add(egui::Slider::new(&mut self.procedural.synth.freq, 20.0..=4000.0).text("频率 Hz"));
        ui.add(egui::Slider::new(&mut self.procedural.synth.dur, 0.02..=2.0).text("时长 s"));
        ui.add(egui::Slider::new(&mut self.procedural.synth.attack, 0.0..=1.0).text("attack"));
        ui.add(egui::Slider::new(&mut self.procedural.synth.decay, 0.0..=1.0).text("decay"));
        ui.add(egui::Slider::new(&mut self.procedural.synth.sustain, 0.0..=1.0).text("sustain"));
        ui.add(egui::Slider::new(&mut self.procedural.synth.release, 0.0..=1.0).text("release"));
        ui.add(egui::Slider::new(&mut self.procedural.synth.gain, 0.0..=1.0).text("增益"));
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.procedural.synth_use_filter, "低通滤波");
            let mut cutoff = self
                .procedural
                .synth
                .filter
                .map_or(1200.0, |filter| filter.cutoff);
            if ui
                .add_enabled(
                    self.procedural.synth_use_filter,
                    egui::Slider::new(&mut cutoff, 200.0..=8000.0)
                        .text("截止 Hz")
                        .logarithmic(true),
                )
                .changed()
            {
                self.procedural.synth.filter = Some(FilterSpec {
                    kind: FilterKind::LowPass,
                    cutoff,
                });
            }
        });
        if self.procedural.synth_use_filter {
            if self.procedural.synth.filter.is_none() {
                self.procedural.synth.filter = Some(FilterSpec {
                    kind: FilterKind::LowPass,
                    cutoff: 1200.0,
                });
            }
        } else {
            self.procedural.synth.filter = None;
        }

        let spec = self.procedural.synth;
        if self.procedural.synth_last != Some(spec) {
            match render_samples(&spec, SYNTH_SAMPLE_RATE) {
                Ok(samples) => {
                    self.procedural.synth_info = format!(
                        "{:.2}s · {} 采样 · 峰值 {:.2} · RMS {:.3}",
                        samples.len() as f32 / SYNTH_SAMPLE_RATE as f32,
                        samples.len(),
                        peak(&samples),
                        rms(&samples),
                    );
                    self.procedural.synth_samples = samples;
                }
                Err(error) => {
                    self.procedural.synth_samples.clear();
                    self.procedural.synth_info = format!("合成失败：{error}");
                }
            }
            self.procedural.synth_last = Some(spec);
        }

        ui.label(&self.procedural.synth_info);
        ui.separator();
        let width = ui.available_width().clamp(120.0, 520.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 160.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, egui::Color32::from_black_alpha(200));
        painter.line_segment(
            [
                egui::pos2(rect.left(), rect.center().y),
                egui::pos2(rect.right(), rect.center().y),
            ],
            egui::Stroke::new(1.0_f32, egui::Color32::from_gray(70)),
        );
        let samples = &self.procedural.synth_samples;
        if !samples.is_empty() {
            let step = (samples.len() as f32 / rect.width().max(1.0)).max(1.0) as usize;
            let mut points = Vec::with_capacity(samples.len() / step + 1);
            let mut index = 0;
            while index < samples.len() {
                let x = rect.left() + rect.width() * (index as f32 / samples.len() as f32);
                let y = rect.center().y - samples[index] * rect.height() * 0.46;
                points.push(egui::pos2(x, y));
                index += step;
            }
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(120, 220, 160)),
            ));
        }
    }

    fn draw_procedural_solid(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        egui::ComboBox::from_label("几何")
            .selected_text(self.procedural.solid_kind.token())
            .show_ui(ui, |ui| {
                for kind in SOLID_KINDS {
                    ui.selectable_value(&mut self.procedural.solid_kind, kind, kind.token());
                }
            });
        ui.add(egui::Slider::new(&mut self.procedural.solid_size, 0.4..=4.0).text("尺寸"));
        ui.horizontal(|ui| {
            color_edit(ui, "材质色 A", &mut self.procedural.mat_colors[0]);
            color_edit(ui, "材质色 B", &mut self.procedural.mat_colors[1]);
        });

        let signature = (
            self.procedural.solid_kind,
            (self.procedural.solid_size * 100.0) as u32,
            self.procedural.mat_colors,
        );
        if self.procedural.solid_last != Some(signature) {
            match self.gpu.clone() {
                Some((device, queue)) => {
                    match self.render_solid_preview(&device, &queue, context) {
                        Ok(()) => {
                            self.procedural.solid_info = format!(
                                "离屏 3D · {} · {PREVIEW_W}×{PREVIEW_H}",
                                self.procedural.solid_kind.token()
                            );
                        }
                        Err(error) => {
                            self.procedural.preview_tex = None;
                            self.procedural.solid_info = format!("渲染失败：{error}");
                        }
                    }
                }
                None => {
                    self.procedural.preview_tex = None;
                    self.procedural.solid_info =
                        "需要 GPU（当前无设备，无法渲染离屏 3D）".to_owned();
                }
            }
            self.procedural.solid_last = Some(signature);
        }

        ui.label(&self.procedural.solid_info);
        ui.separator();
        if let Some(texture) = &self.procedural.preview_tex {
            let max = ui.available_size();
            ui.add(egui::Image::new(texture).max_size(egui::vec2(max.x, max.y * 0.86)));
        } else {
            ui.weak("（立体预览需要真实 GPU 设备；无头/无窗口环境显示不到图）");
        }
    }

    /// 用宿主 `gpu` 的专属离屏渲染器把 primitive + 生成材质画一帧，回读成 egui 图像。
    fn render_solid_preview(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        context: &egui::Context,
    ) -> Result<(), String> {
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        // 把渲染器**取出**成局部量，避免与同结构里的 target / preview_tex 抢借用。
        let mut renderer = match self.procedural.preview_renderer.take() {
            Some(renderer) => renderer,
            None => {
                let mut renderer = Renderer::new_offscreen(
                    device.clone(),
                    queue.clone(),
                    Some(root),
                    false,
                    false,
                )
                .map_err(|error| error.to_string())?;
                renderer.set_render_settings(
                    RenderSettings::default()
                        .with_pbr(true)
                        .with_hdr(ToggleMode::On),
                );
                renderer.set_post_fx(0.0, 0.0);
                renderer.set_water_refraction_gain(0.0);
                renderer
            }
        };
        if self.procedural.preview_target.is_none() {
            self.procedural.preview_target =
                Some(device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("rustlua-procedural-solid"),
                    size: wgpu::Extent3d {
                        width: PREVIEW_W,
                        height: PREVIEW_H,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                }));
        }

        // 材质贴图：用两个材质色做一张棋盘，登记成哨兵键供材质解析。
        let mut material_spec = TextureSpec::default_for(TextureKind::Checker);
        material_spec.size = 64;
        material_spec.scale = 4.0;
        material_spec.colors = self.procedural.mat_colors.to_vec();
        let material = material_spec
            .generate()
            .map_err(|error| error.to_string())?;
        renderer.register_generated_texture(SOLID_MATERIAL_KEY, material);

        let spec = self.procedural.solid_kind.build(self.procedural.solid_size);
        let (min, max) = spec.bounds();
        let drawable = Scene3dDrawable {
            entity: rustlua_ecs::spatial_transform::SceneEntityId(1),
            name: "ProceduralSolid".into(),
            mesh_path: spec.mesh_path().into(),
            material: Some(SOLID_MATERIAL_KEY.into()),
            mesh_handle: None,
            model: Mat4::identity(),
            palette: Vec::new(),
            instances: Vec::new(),
            bounds: Scene3dBounds {
                local_aabb: Aabb3::new(
                    Vec3::new(min[0], min[1], min[2]),
                    Vec3::new(max[0], max[1], max[2]),
                )
                .map_err(|error| error.to_string())?,
                source: BoundsSource::Fallback,
            },
        };
        let viewport =
            Viewport::new(PREVIEW_W as f32, PREVIEW_H as f32).map_err(|error| error.to_string())?;
        let camera = Camera3D::perspective(
            Vec3::new(2.4, 1.9, 2.4),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            0.1,
            100.0,
        )
        .map_err(|error| error.to_string())?;
        let view = RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR);
        renderer.set_frame_views(FrameViews {
            main: Some(view),
            overlay: None,
        });
        let scene = Scene3d {
            camera,
            view,
            light: Light::default(),
            point_lights: Vec::new(),
            drawables: vec![drawable],
        };
        let target = self
            .procedural
            .preview_target
            .as_ref()
            .expect("preview target ensured");
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mesh = empty_scene_mesh();
        renderer.render_into_texture(
            &mesh,
            Some(&scene),
            &target_view,
            PREVIEW_W,
            PREVIEW_H,
            true,
        );
        let pixels = renderer
            .copy_texture_to_cpu(target, PREVIEW_W, PREVIEW_H)
            .map_err(|error| error.to_string())?;
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [PREVIEW_W as usize, PREVIEW_H as usize],
            &pixels,
        );
        self.procedural.preview_tex =
            Some(context.load_texture("procedural_solid", image, egui::TextureOptions::LINEAR));
        self.procedural.preview_renderer = Some(renderer);
        Ok(())
    }
}

/// 色块编辑：`[u8;4]` ⇄ `Color32`。
fn color_edit(ui: &mut egui::Ui, label: &str, color: &mut [u8; 4]) {
    let mut value = egui::Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
    if ui.color_edit_button_srgba(&mut value).changed() {
        *color = value.to_array();
    }
    ui.label(label);
}

/// 读编辑器自带的中文字体（第一个存在的候选路径）；读不到返回 `None`（面板会提示）。
fn load_editor_font() -> Option<FontArc> {
    let path = crate::util::cjk_font_candidates()
        .into_iter()
        .find(|candidate| candidate.is_file())?;
    let bytes = std::fs::read(path).ok()?;
    load_font(bytes)
}

/// 编辑器手写空 2D 网格：立体预览只用 3D drawables，2D 侧留空。
fn empty_scene_mesh() -> SceneMesh {
    SceneMesh {
        spines: rustlua_render::spine_mesh::SpineMesh::default(),
        vertices: Vec::new(),
        draws: Vec::new(),
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: Vec::new(),
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{EditorApp, Tab};
    use eframe::egui;
    use rustlua_infra::logging::LogSink;

    fn app(gpu: Option<(wgpu::Device, wgpu::Queue)>) -> EditorApp {
        let root = std::env::temp_dir().join(format!("rustlua-editor-proc-{}", std::process::id()));
        let project = editor::ProjectInfo {
            root,
            version: "test".to_owned(),
            directories: Vec::new(),
        };
        let mut app = EditorApp::new(project, gpu, LogSink::new(64));
        app.project_loaded = true;
        app
    }

    fn draw(app: &mut EditorApp) {
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            app.draw_procedural_panel(context);
        });
    }

    /// 无头离屏设备（fallback adapter，swiftshader）；无 GPU 时返回 None，测试跳过。
    fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rustlua-editor-procedural"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()
    }

    /// 「入口可修改可预览」的无头凭证（无 GPU）：贴图/文字/音效三页各画一帧都要产出内容；
    /// 立体页在无 GPU 时降级为提示（不 panic、不凭空产图）。
    #[test]
    fn panel_generates_cpu_previews_without_gpu() {
        let mut app = app(None);
        app.procedural.show = true;

        draw(&mut app);
        assert!(
            app.procedural.tex_tex.is_some(),
            "贴图页应生成预览纹理：{}",
            app.procedural.tex_info
        );

        app.procedural.tab = Tab::Text;
        draw(&mut app);
        assert!(
            app.procedural.font.is_some(),
            "编辑器自带字体应能加载：{}",
            app.procedural.text_info
        );
        assert!(
            app.procedural.text_tex.is_some(),
            "文字页应生成预览纹理：{}",
            app.procedural.text_info
        );

        app.procedural.tab = Tab::Synth;
        draw(&mut app);
        assert!(
            !app.procedural.synth_samples.is_empty(),
            "合成页应渲染出采样：{}",
            app.procedural.synth_info
        );

        app.procedural.tab = Tab::Solid;
        draw(&mut app);
        assert!(
            app.procedural.preview_tex.is_none(),
            "无 GPU 时立体页不该凭空产出图"
        );
        assert!(
            app.procedural.solid_info.contains("GPU"),
            "无 GPU 应给出提示：{}",
            app.procedural.solid_info
        );
    }

    /// 立体页的真渲染：注入离屏设备后，参数化 primitive + 生成材质要真的画出一张图。
    #[test]
    fn solid_tab_renders_offscreen_with_gpu() {
        let Some((device, queue)) = test_device() else {
            return;
        };
        let mut app = app(Some((device, queue)));
        app.procedural.show = true;
        app.procedural.tab = Tab::Solid;
        draw(&mut app);
        assert!(
            app.procedural.preview_tex.is_some(),
            "有 GPU 时立体页应渲染出预览图：{}",
            app.procedural.solid_info
        );
    }
}
