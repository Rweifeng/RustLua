//! 模型预览：资源面板选中 `.glb` / `.gltf` 时，属性面板就地显示可旋转/缩放的模型。
//!
//! 三条口径，与 `texture_preview` 同族：
//! 1. **只读**：旋转与缩放只是本窗的观察参数，不写工程文件，也不改资源导入设置；
//! 2. **CPU 软光栅，不上 wgpu**：预览结果要能在无 GPU 的门里逐像素比较，而且同一份
//!    模型 + 同一组视角必须给出同一张图（缓存因此可以只按参数作废）；
//! 3. **同步加载、按路径缓存**：选中是显式用户动作，一次只解析一个 glb，切走即丢弃
//!    （与贴图预览"同步解码、一次一张"同一取舍；换来的是不引入后台线程与代际号）。
//!
//! 预览按 glTF 的节点树烘焙世界变换，所以有层级/缩放的模型摆得对；蒙皮只画绑定姿势
//! （预览不做骨骼求值，动画要看时间线面板）。

use std::collections::HashMap;
use std::path::Path;

use eframe::egui;
use rustlua_assets::gltf_asset::{GltfAsset, GltfPrimitive};
use rustlua_math::math3d::{Aabb3, Mat4, Quat, Vec3};

use crate::EditorApp;

/// 预览画布的逻辑尺寸（软光栅的目标分辨率；显示时按原尺寸贴图，不随面板拉伸）。
pub(crate) const PREVIEW_SIZE: (u32, u32) = (320, 240);
const DEFAULT_YAW: f32 = 35.0;
const DEFAULT_PITCH: f32 = 18.0;
/// 没有材质时的基色（线性空间的中性灰）。
const DEFAULT_COLOR: [f32; 3] = [0.80, 0.80, 0.84];

/// 预览窗状态（挂在 `EditorApp` 上；只缓存当前这一个模型）。
pub(crate) struct ModelPreviewState {
    /// 正在预览的模型（工程内相对路径，与资源索引同一口径）。
    pub(crate) relative: Option<String>,
    /// 已经解析过的模型属于哪条路径：失败也会记住，避免每帧重试同一个坏文件。
    model_for: Option<String>,
    model: Option<LoadedModel>,
    /// 水平旋转（度，绕 Y）。
    pub(crate) yaw: f32,
    /// 俯仰（度，绕 X）。
    pub(crate) pitch: f32,
    /// 观察缩放（1.0 = 按包围球装进画布；不是模型的缩放）。
    pub(crate) scale: f32,
    texture: Option<egui::TextureHandle>,
    render_key: Option<RenderKey>,
    /// 解析失败原因（与"还没选中模型"区分开）。
    pub(crate) error: Option<String>,
    /// 加载成功后的规模读数（信息行用）。
    pub(crate) stats: ModelStats,
}

impl Default for ModelPreviewState {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelPreviewState {
    pub(crate) fn new() -> Self {
        Self {
            relative: None,
            model_for: None,
            model: None,
            yaw: DEFAULT_YAW,
            pitch: DEFAULT_PITCH,
            scale: 1.0,
            texture: None,
            render_key: None,
            error: None,
            stats: ModelStats::default(),
        }
    }

    /// 切换/关闭工程：正在预览的路径属于旧工程根，整份作废。
    pub(crate) fn clear(&mut self) {
        *self = Self::new();
    }

    /// 换模型：丢掉上一个的几何体与纹理，视角参数保留（连续比较几个模型更顺手）。
    fn retarget(&mut self, relative: &str) {
        self.relative = Some(relative.to_owned());
        self.model_for = None;
        self.model = None;
        self.texture = None;
        self.render_key = None;
        self.error = None;
        self.stats = ModelStats::default();
    }

    fn reset_view(&mut self) {
        self.yaw = DEFAULT_YAW;
        self.pitch = DEFAULT_PITCH;
        self.scale = 1.0;
    }

    /// 测试探针：当前是否已上传一张可画的预览纹理。
    #[cfg(test)]
    pub(crate) fn has_texture(&self) -> bool {
        self.texture.is_some()
    }
}

/// 规模读数（信息行；也用作"切了模型就必须换一组数"的可观测量）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModelStats {
    pub(crate) meshes: usize,
    pub(crate) vertices: usize,
    pub(crate) triangles: usize,
    pub(crate) source_bytes: u64,
}

/// 渲染缓存的键：视角或尺寸变一条就重画，否则直接复用上一张纹理。
#[derive(Clone, Debug, PartialEq, Eq)]
struct RenderKey {
    relative: String,
    yaw: u32,
    pitch: u32,
    scale: u32,
    size: (u32, u32),
}

impl RenderKey {
    fn new(state: &ModelPreviewState, relative: &str) -> Self {
        Self {
            relative: relative.to_owned(),
            yaw: state.yaw.to_bits(),
            pitch: state.pitch.to_bits(),
            scale: state.scale.to_bits(),
            size: PREVIEW_SIZE,
        }
    }
}

struct PreviewTriangle {
    positions: [[f32; 3]; 3],
    normals: [[f32; 3]; 3],
    uvs: [[f32; 2]; 3],
    /// 线性空间基色（材质 `base_color_factor`；无材质时用 [`DEFAULT_COLOR`]）。
    color: [f32; 3],
    /// `BaseColor` 贴图在 [`LoadedModel::textures`] 里的槽位。
    texture: Option<usize>,
}

struct PreviewTexture {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

struct LoadedModel {
    triangles: Vec<PreviewTriangle>,
    textures: Vec<PreviewTexture>,
    center: Vec3,
    /// 包围球半径：旋转时用它装镜头，任何角度都不裁边。
    radius: f32,
    stats: ModelStats,
}

struct MaterialView {
    color: [f32; 3],
    texture: Option<usize>,
}

impl EditorApp {
    /// 属性面板里的模型段：信息行 + 视角控件 + 可拖动的预览图。
    pub(crate) fn draw_model_preview(&mut self, ui: &mut egui::Ui, relative: &str) {
        if self.model_preview.relative.as_deref() != Some(relative) {
            self.model_preview.retarget(relative);
        }
        self.load_model_preview(relative);

        ui.label(egui::RichText::new("模型预览").strong());
        if let Some(error) = &self.model_preview.error {
            ui.colored_label(egui::Color32::RED, format!("加载失败：{error}"));
            if ui.button("重新加载").clicked() {
                self.model_preview.model_for = None;
                ui.ctx().request_repaint();
            }
            return;
        }

        let stats = self.model_preview.stats.clone();
        ui.horizontal_wrapped(|ui| {
            ui.weak(format!(
                "{} 个网格 · {} 顶点 · {} 三角形 · 源文件 {}",
                stats.meshes,
                stats.vertices,
                stats.triangles,
                super::texture_preview::byte_text(stats.source_bytes)
            ));
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("水平");
            ui.add(egui::Slider::new(&mut self.model_preview.yaw, -180.0..=180.0).suffix("°"));
            ui.label("俯仰");
            ui.add(egui::Slider::new(&mut self.model_preview.pitch, -89.0..=89.0).suffix("°"));
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("缩放");
            ui.add(egui::Slider::new(&mut self.model_preview.scale, 0.25..=4.0).logarithmic(true));
            if ui.button("重置视角").clicked() {
                self.model_preview.reset_view();
                ui.ctx().request_repaint();
            }
            if ui
                .button("重新加载")
                .on_hover_text("外部换过这个 glb 之后重读一次磁盘")
                .clicked()
            {
                self.model_preview.model_for = None;
                self.model_preview.texture = None;
                self.model_preview.render_key = None;
                ui.ctx().request_repaint();
            }
        });

        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(PREVIEW_SIZE.0 as f32, PREVIEW_SIZE.1 as f32),
            egui::Sense::drag(),
        );
        if response.dragged() {
            let delta = response.drag_delta();
            self.model_preview.yaw = wrap_angle(self.model_preview.yaw + delta.x * 0.6);
            self.model_preview.pitch =
                (self.model_preview.pitch - delta.y * 0.5).clamp(-89.0, 89.0);
            ui.ctx().request_repaint();
        }
        self.refresh_model_texture(ui.ctx());
        let painter = ui.painter_at(rect);
        match &self.model_preview.texture {
            Some(texture) => {
                painter.image(
                    texture.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                painter.rect_filled(rect, 0.0, egui::Color32::from_gray(40));
            }
        }
        response.on_hover_text("拖动旋转，滑杆改水平/俯仰/缩放；只影响预览，不改资源文件");
    }

    /// 按路径解析一次；失败也记在 `model_for` 上，坏文件不会每帧重试。
    fn load_model_preview(&mut self, relative: &str) {
        if self.model_preview.model_for.as_deref() == Some(relative) {
            return;
        }
        self.model_preview.model_for = Some(relative.to_owned());
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        match load_model(&root.join(relative)) {
            Ok(model) => {
                self.model_preview.stats = model.stats.clone();
                self.model_preview.model = Some(model);
                self.model_preview.error = None;
            }
            Err(error) => {
                self.model_preview.stats = ModelStats::default();
                self.model_preview.model = None;
                self.model_preview.error = Some(error);
            }
        }
        self.model_preview.texture = None;
        self.model_preview.render_key = None;
    }

    /// 视角/尺寸有变化才重画并重传纹理（拖动滑杆时每帧最多画一次）。
    fn refresh_model_texture(&mut self, context: &egui::Context) {
        let Some(relative) = self.model_preview.relative.clone() else {
            return;
        };
        let Some(model) = self.model_preview.model.as_ref() else {
            self.model_preview.texture = None;
            self.model_preview.render_key = None;
            return;
        };
        let key = RenderKey::new(&self.model_preview, &relative);
        if self.model_preview.render_key.as_ref() == Some(&key)
            && self.model_preview.texture.is_some()
        {
            return;
        }
        let image = render_model_image(
            model,
            self.model_preview.yaw,
            self.model_preview.pitch,
            self.model_preview.scale,
            PREVIEW_SIZE,
        );
        match self.model_preview.texture.as_mut() {
            Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
            None => {
                self.model_preview.texture = Some(context.load_texture(
                    format!("model_preview:{relative}"),
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        self.model_preview.render_key = Some(key);
    }
}

fn wrap_angle(value: f32) -> f32 {
    (value + 180.0).rem_euclid(360.0) - 180.0
}

// ── 解析：glTF → 世界空间三角形 ─────────────────────────────────────────────

fn load_model(absolute: &Path) -> Result<LoadedModel, String> {
    let asset = GltfAsset::load(absolute).map_err(|error| error.to_string())?;
    let source_bytes = std::fs::metadata(absolute).map_or(0, |meta| meta.len());
    build_model(&asset, source_bytes)
}

/// 把解析好的 glTF 烘成世界空间三角形（节点树里的 TRS 在这里生效）。
fn build_model(asset: &GltfAsset, source_bytes: u64) -> Result<LoadedModel, String> {
    let (materials, textures) = material_views(asset);
    let mut build = BuildState {
        asset,
        materials: &materials,
        visited: vec![false; asset.nodes.len()],
        triangles: Vec::new(),
        vertices: 0,
        triangle_count: 0,
    };
    let mut has_parent = vec![false; asset.nodes.len()];
    for node in &asset.nodes {
        for &child in &node.children {
            if let Some(flag) = has_parent.get_mut(child) {
                *flag = true;
            }
        }
    }
    let roots: Vec<usize> = (0..asset.nodes.len())
        .filter(|index| !has_parent[*index])
        .collect();
    if roots.is_empty() && !asset.nodes.is_empty() {
        // 数据成环（没有根）：从 0 号走一遍，`visited` 保证不会无限递归。
        build.walk(0, Mat4::identity());
    } else {
        for root in roots {
            build.walk(root, Mat4::identity());
        }
    }
    let BuildState {
        triangles,
        vertices,
        triangle_count,
        ..
    } = build;
    if triangles.is_empty() {
        return Err("模型里没有可预览的三角形".to_owned());
    }
    let points: Vec<Vec3> = triangles
        .iter()
        .flat_map(|triangle| {
            triangle
                .positions
                .iter()
                .map(|point| Vec3::new(point[0], point[1], point[2]))
        })
        .collect();
    let bounds =
        Aabb3::from_points(&points).ok_or_else(|| "模型的顶点坐标不是有限值".to_owned())?;
    let sphere = bounds.bounding_sphere();
    Ok(LoadedModel {
        triangles,
        textures,
        center: sphere.center,
        radius: sphere.radius.max(1e-4),
        stats: ModelStats {
            meshes: asset.meshes.len(),
            vertices,
            triangles: triangle_count,
            source_bytes,
        },
    })
}

/// 每个材质一条视图；同一张 `BaseColor` 贴图只留一份像素。
fn material_views(asset: &GltfAsset) -> (Vec<MaterialView>, Vec<PreviewTexture>) {
    let mut views = Vec::with_capacity(asset.materials.len());
    let mut textures: Vec<PreviewTexture> = Vec::new();
    let mut by_texture: HashMap<usize, usize> = HashMap::new();
    for material in &asset.materials {
        let mut texture = None;
        if let Some(image) = &material.base_color_image {
            let slot = match material.base_color_texture {
                Some(index) => *by_texture.entry(index).or_insert_with(|| {
                    textures.push(PreviewTexture {
                        width: image.width,
                        height: image.height,
                        rgba: image.rgba.clone(),
                    });
                    textures.len() - 1
                }),
                None => {
                    textures.push(PreviewTexture {
                        width: image.width,
                        height: image.height,
                        rgba: image.rgba.clone(),
                    });
                    textures.len() - 1
                }
            };
            texture = Some(slot);
        }
        let [red, green, blue, _] = material.base_color_factor;
        views.push(MaterialView {
            color: [red, green, blue],
            texture,
        });
    }
    (views, textures)
}

struct BuildState<'a> {
    asset: &'a GltfAsset,
    materials: &'a [MaterialView],
    visited: Vec<bool>,
    triangles: Vec<PreviewTriangle>,
    vertices: usize,
    triangle_count: usize,
}

impl BuildState<'_> {
    fn walk(&mut self, node: usize, parent: Mat4) {
        let Some(node_data) = self.asset.nodes.get(node) else {
            return;
        };
        if self.visited[node] {
            return;
        }
        self.visited[node] = true;
        let [tx, ty, tz] = node_data.translation;
        let [rx, ry, rz, rw] = node_data.rotation;
        let [sx, sy, sz] = node_data.scale;
        let local = Mat4::translation(Vec3::new(tx, ty, tz))
            * Mat4::from_quat(Quat {
                x: rx,
                y: ry,
                z: rz,
                w: rw,
            })
            * Mat4::scale(Vec3::new(sx, sy, sz));
        let world = parent * local;
        let mesh_index = node_data.mesh;
        let children = node_data.children.clone();
        if let Some(mesh_index) = mesh_index
            && let Some(mesh) = self.asset.meshes.get(mesh_index)
        {
            for primitive in &mesh.primitives {
                let material = primitive
                    .material
                    .and_then(|index| self.materials.get(index));
                push_primitive(
                    &mut self.triangles,
                    &mut self.vertices,
                    &mut self.triangle_count,
                    primitive,
                    world,
                    material,
                );
            }
        }
        for child in children {
            self.walk(child, world);
        }
    }
}

fn push_primitive(
    out: &mut Vec<PreviewTriangle>,
    vertices: &mut usize,
    triangle_count: &mut usize,
    primitive: &GltfPrimitive,
    world: Mat4,
    material: Option<&MaterialView>,
) {
    let count = primitive.positions.len();
    *vertices += count;
    let color = material.map_or(DEFAULT_COLOR, |view| view.color);
    let texture = material.and_then(|view| view.texture);
    let has_normals = primitive.normals.len() == count;
    let has_uvs = primitive.texcoords.len() == count;
    for triangle in primitive.indices.chunks_exact(3) {
        let indices = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        if indices.iter().any(|index| *index >= count) {
            continue;
        }
        let mut positions = [[0.0; 3]; 3];
        let mut normals = [[0.0; 3]; 3];
        let mut uvs = [[0.0; 2]; 3];
        let mut finite = true;
        for (slot, index) in indices.iter().enumerate() {
            let Ok(point) = world.transform_point(primitive.positions[*index]) else {
                finite = false;
                break;
            };
            positions[slot] = [point.x, point.y, point.z];
            normals[slot] = if has_normals {
                rotate_normal(world, primitive.normals[*index])
            } else {
                Vec3::ZERO
            }
            .to_array();
            uvs[slot] = if has_uvs {
                primitive.texcoords[*index]
            } else {
                [0.0, 0.0]
            };
        }
        if !finite {
            continue;
        }
        if normals.iter().all(|normal| *normal == [0.0; 3]) {
            // 作者没写法线（或全退化）：用面法线顶着，正反两面都亮。
            let face = face_normal(&positions).to_array();
            normals = [face; 3];
        }
        if normals.iter().all(|normal| *normal == [0.0; 3]) {
            // 面积为零的三角形画不出东西，留着只会拖慢光栅。
            continue;
        }
        out.push(PreviewTriangle {
            positions,
            normals,
            uvs,
            color,
            texture,
        });
        *triangle_count += 1;
    }
}

fn rotate_normal(matrix: Mat4, normal: Vec3) -> Vec3 {
    let rotated = Vec3::new(
        matrix.columns[0][0] * normal.x
            + matrix.columns[1][0] * normal.y
            + matrix.columns[2][0] * normal.z,
        matrix.columns[0][1] * normal.x
            + matrix.columns[1][1] * normal.y
            + matrix.columns[2][1] * normal.z,
        matrix.columns[0][2] * normal.x
            + matrix.columns[1][2] * normal.y
            + matrix.columns[2][2] * normal.z,
    );
    rotated.normalized().unwrap_or(Vec3::ZERO)
}

fn face_normal(positions: &[[f32; 3]; 3]) -> Vec3 {
    let [a, b, c] = *positions;
    let a = Vec3::new(a[0], a[1], a[2]);
    let b = Vec3::new(b[0], b[1], b[2]);
    let c = Vec3::new(c[0], c[1], c[2]);
    (b - a).cross(c - a).normalized().unwrap_or(Vec3::ZERO)
}

// ── 软光栅 ───────────────────────────────────────────────────────────────

/// 把模型按视角渲染成一张 RGBA8 图（纯函数：同输入必同输出，门直接比像素）。
fn render_model_image(
    model: &LoadedModel,
    yaw: f32,
    pitch: f32,
    scale: f32,
    size: (u32, u32),
) -> egui::ColorImage {
    let width = size.0.max(1) as usize;
    let height = size.1.max(1) as usize;
    let rotation = Mat4::from_quat(
        Quat::from_axis_angle(Vec3::Y, yaw.to_radians()).unwrap_or_else(|_| Quat::identity())
            * Quat::from_axis_angle(Vec3::X, pitch.to_radians())
                .unwrap_or_else(|_| Quat::identity()),
    );
    let view = View {
        rotation,
        // 0.42 留一圈边：包围球装进短边后，任何角度旋转都不会贴边被裁。
        fit: 0.42 * width.min(height) as f32 / model.radius * scale.max(0.01),
        center: model.center,
        light: Vec3::new(-0.35, 0.62, 0.78).normalized().unwrap_or(Vec3::Y),
        textures: &model.textures,
    };
    let mut framebuffer = Framebuffer::new(width, height);
    for triangle in &model.triangles {
        framebuffer.rasterize(&view, triangle);
    }
    framebuffer.into_image()
}

struct View<'a> {
    rotation: Mat4,
    fit: f32,
    center: Vec3,
    light: Vec3,
    textures: &'a [PreviewTexture],
}

struct Framebuffer {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
    depth: Vec<f32>,
}

impl Framebuffer {
    fn new(width: usize, height: usize) -> Self {
        let mut pixels = vec![0u8; width * height * 4];
        for y in 0..height {
            let [red, green, blue] = background_color(y, height);
            for x in 0..width {
                let index = (y * width + x) * 4;
                pixels[index] = red;
                pixels[index + 1] = green;
                pixels[index + 2] = blue;
                pixels[index + 3] = 255;
            }
        }
        Self {
            width,
            height,
            pixels,
            // 正交投影下 z 越大小越近；初值取极小值，任何三角形都能盖上去。
            depth: vec![f32::NEG_INFINITY; width * height],
        }
    }

    fn rasterize(&mut self, view: &View<'_>, triangle: &PreviewTriangle) {
        let mut screen = [[0.0f32; 2]; 3];
        let mut depth = [0.0f32; 3];
        let mut normals = [Vec3::ZERO; 3];
        for slot in 0..3 {
            let point = triangle.positions[slot];
            let local = Vec3::new(
                point[0] - view.center.x,
                point[1] - view.center.y,
                point[2] - view.center.z,
            );
            let rotated = rotate_vector(view.rotation, local);
            screen[slot] = [
                self.width as f32 * 0.5 + rotated.x * view.fit,
                self.height as f32 * 0.5 - rotated.y * view.fit,
            ];
            depth[slot] = rotated.z;
            normals[slot] = rotate_vector(
                view.rotation,
                Vec3::new(
                    triangle.normals[slot][0],
                    triangle.normals[slot][1],
                    triangle.normals[slot][2],
                ),
            );
        }
        let [a, b, c] = screen;
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
        if !area.is_finite() || area.abs() < 1e-6 {
            return;
        }
        let min_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
        let max_x = (a[0].max(b[0]).max(c[0]).ceil() as i64).min(self.width as i64 - 1);
        let min_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
        let max_y = (a[1].max(b[1]).max(c[1]).ceil() as i64).min(self.height as i64 - 1);
        if max_x < min_x as i64 || max_y < min_y as i64 {
            return;
        }
        let inverse_area = 1.0 / area;
        for y in min_y..=max_y as usize {
            for x in min_x..=max_x as usize {
                let sample = [x as f32 + 0.5, y as f32 + 0.5];
                let weights = [
                    edge(b, c, sample) * inverse_area,
                    edge(c, a, sample) * inverse_area,
                    edge(a, b, sample) * inverse_area,
                ];
                if weights.iter().any(|weight| *weight < 0.0) {
                    continue;
                }
                let z = weights[0] * depth[0] + weights[1] * depth[1] + weights[2] * depth[2];
                let index = y * self.width + x;
                if z <= self.depth[index] {
                    continue;
                }
                let mut normal = Vec3::new(
                    weights[0] * normals[0].x
                        + weights[1] * normals[1].x
                        + weights[2] * normals[2].x,
                    weights[0] * normals[0].y
                        + weights[1] * normals[1].y
                        + weights[2] * normals[2].y,
                    weights[0] * normals[0].z
                        + weights[1] * normals[1].z
                        + weights[2] * normals[2].z,
                );
                normal = normal.normalized().unwrap_or(Vec3::new(0.0, 0.0, 1.0));
                if normal.z < 0.0 {
                    // 双面着色：背面也按朝向镜头的那一侧受光，避免半个模型是黑的。
                    normal = -normal;
                }
                let albedo = match triangle.texture {
                    Some(slot) => {
                        let uv = [
                            weights[0] * triangle.uvs[0][0]
                                + weights[1] * triangle.uvs[1][0]
                                + weights[2] * triangle.uvs[2][0],
                            weights[0] * triangle.uvs[0][1]
                                + weights[1] * triangle.uvs[1][1]
                                + weights[2] * triangle.uvs[2][1],
                        ];
                        view.textures
                            .get(slot)
                            .map_or(triangle.color, |texture| sample_texture(texture, uv))
                    }
                    None => triangle.color,
                };
                let lambert = normal.dot(view.light).max(0.0);
                let rim = (1.0 - normal.z.max(0.0)).powi(3) * 0.18;
                let intensity = 0.30 + 0.80 * lambert + rim;
                self.depth[index] = z;
                let pixel = index * 4;
                self.pixels[pixel] = linear_to_srgb(albedo[0] * intensity);
                self.pixels[pixel + 1] = linear_to_srgb(albedo[1] * intensity);
                self.pixels[pixel + 2] = linear_to_srgb(albedo[2] * intensity);
                self.pixels[pixel + 3] = 255;
            }
        }
    }

    fn into_image(self) -> egui::ColorImage {
        egui::ColorImage::from_rgba_unmultiplied([self.width, self.height], &self.pixels)
    }
}

/// 半平面判别式：`(b-a) × (p-a)`。三条边同号 = 点在三角形内（两种缠绕都成立）。
fn edge(a: [f32; 2], b: [f32; 2], point: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0])
}

fn rotate_vector(matrix: Mat4, value: Vec3) -> Vec3 {
    Vec3::new(
        matrix.columns[0][0] * value.x
            + matrix.columns[1][0] * value.y
            + matrix.columns[2][0] * value.z,
        matrix.columns[0][1] * value.x
            + matrix.columns[1][1] * value.y
            + matrix.columns[2][1] * value.z,
        matrix.columns[0][2] * value.x
            + matrix.columns[1][2] * value.y
            + matrix.columns[2][2] * value.z,
    )
}

/// 画布底色（纵向渐变；测试靠它区分"有模型"与"只有底板"）。
fn background_color(y: usize, height: usize) -> [u8; 3] {
    let t = (y as f32 / height.max(1) as f32).clamp(0.0, 1.0);
    let top = [30.0, 32.0, 38.0];
    let bottom = [46.0, 48.0, 56.0];
    [0, 1, 2].map(|channel| (top[channel] + (bottom[channel] - top[channel]) * t) as u8)
}

fn sample_texture(texture: &PreviewTexture, uv: [f32; 2]) -> [f32; 3] {
    let width = texture.width as usize;
    let height = texture.height as usize;
    if width == 0 || height == 0 || texture.rgba.len() < width * height * 4 {
        return DEFAULT_COLOR;
    }
    let u = uv[0].rem_euclid(1.0) * width as f32 - 0.5;
    let v = uv[1].rem_euclid(1.0) * height as f32 - 0.5;
    let x0 = u.floor();
    let y0 = v.floor();
    let fx = u - x0;
    let fy = v - y0;
    let sample = |x: f32, y: f32, channel: usize| -> f32 {
        let x = (x as i64).rem_euclid(width as i64) as usize;
        let y = (y as i64).rem_euclid(height as i64) as usize;
        srgb_to_linear(texture.rgba[(y * width + x) * 4 + channel])
    };
    let mut color = [0.0f32; 3];
    for (channel, value) in color.iter_mut().enumerate() {
        let top = sample(x0, y0, channel) * (1.0 - fx) + sample(x0 + 1.0, y0, channel) * fx;
        let bottom =
            sample(x0, y0 + 1.0, channel) * (1.0 - fx) + sample(x0 + 1.0, y0 + 1.0, channel) * fx;
        *value = top * (1.0 - fy) + bottom * fy;
    }
    color
}

fn srgb_to_linear(value: u8) -> f32 {
    let value = value as f32 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let encoded = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round().clamp(0.0, 255.0) as u8
}

trait Vec3Array {
    fn to_array(self) -> [f32; 3];
}

impl Vec3Array for Vec3 {
    fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorApp;
    use eframe::egui;
    use rustlua_assets::gltf_asset::{GltfMesh, GltfNode};
    use rustlua_infra::logging::LogSink;
    use std::fs;
    use std::path::PathBuf;

    /// 模型夹具住在仓库根的示例工程里（与导入层测试同一份文件，不另造假 glb）。
    fn model_path(name: &str) -> PathBuf {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../examples/game_api/assets/models")
            .join(name);
        assert!(path.is_file(), "缺少模型夹具：{}", path.display());
        path
    }

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-editor-modelprev-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("assets/models")).unwrap();
        root
    }

    fn app(root: PathBuf) -> EditorApp {
        let project = editor::ProjectInfo {
            root,
            version: "test".to_owned(),
            directories: Vec::new(),
        };
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.project_loaded = true;
        app
    }

    /// 画一帧属性面板（无 GPU：`egui::Context::default()` 的纹理表是纯 CPU 的）。
    fn draw_inspector(app: &mut EditorApp) -> String {
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                app.draw_inspector_panel(ui);
            });
        });
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 非底板像素数：底板是 `background_color` 那个渐变，模型落在上面的像素都不等于它。
    fn foreground_pixels(image: &egui::ColorImage) -> usize {
        let [width, height] = image.size;
        image
            .pixels
            .iter()
            .enumerate()
            .filter(|(index, pixel)| {
                let y = index / width;
                let [red, green, blue] = background_color(y, height);
                pixel.r() != red || pixel.g() != green || pixel.b() != blue
            })
            .count()
    }

    #[test]
    fn a_glb_loads_into_world_space_triangles() {
        let model = load_model(&model_path("p3-sphere.glb")).expect("球体应能解析");
        assert!(model.stats.triangles > 0, "{:?}", model.stats);
        assert!(model.stats.vertices > 0, "{:?}", model.stats);
        assert!(model.stats.source_bytes > 0, "源文件体积是真实读数");
        assert!(
            model.radius > 0.0 && model.radius.is_finite(),
            "{:?}",
            model.radius
        );
        let image = render_model_image(&model, 0.0, 0.0, 1.0, (160, 120));
        assert_eq!(image.size, [160, 120]);
        assert!(
            foreground_pixels(&image) > 200,
            "球体应该占掉一块可见面积：{}",
            foreground_pixels(&image)
        );
    }

    /// 节点树上的 TRS 必须在烘焙时生效：同一个三角形挂到"平移 10、缩放 2"的节点下，
    /// 世界坐标就得是 `10 + 2 * 局部坐标`。只解析 mesh 不看节点的实现会在这里判红。
    #[test]
    fn node_translation_and_scale_are_baked_into_the_triangle_positions() {
        let primitive = GltfPrimitive {
            positions: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            normals: Vec::new(),
            texcoords: Vec::new(),
            tangents: Vec::new(),
            indices: vec![0, 1, 2],
            material: None,
            joints: Vec::new(),
            weights: Vec::new(),
        };
        let asset = GltfAsset {
            meshes: vec![GltfMesh {
                name: None,
                primitives: vec![primitive],
            }],
            nodes: vec![GltfNode {
                name: None,
                mesh: Some(0),
                skin: None,
                children: Vec::new(),
                translation: [10.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [2.0, 2.0, 2.0],
            }],
            materials: Vec::new(),
            skins: Vec::new(),
            animations: Vec::new(),
            image_count: 0,
        };
        let model = build_model(&asset, 0).expect("内存里的 glTF 应能烘焙");
        assert_eq!(model.stats.triangles, 1);
        let mut positions: Vec<[f32; 3]> = model.triangles[0]
            .positions
            .iter()
            .map(|point| point.map(|value| (value * 100.0).round() / 100.0))
            .collect();
        positions.sort_by(|left, right| {
            left[0]
                .total_cmp(&right[0])
                .then(left[1].total_cmp(&right[1]))
        });
        assert_eq!(
            positions,
            vec![[10.0, 0.0, 0.0], [10.0, 2.0, 0.0], [12.0, 0.0, 0.0]],
            "节点平移/缩放没进世界坐标"
        );
        assert_eq!(model.center.x, 11.0, "包围球中心也应按世界坐标算");
        assert_eq!(model.center.y, 1.0);
    }

    #[test]
    fn rotating_the_model_changes_the_rendered_pixels() {
        let model = load_model(&model_path("m8-test-triangle.glb")).expect("三角面应能解析");
        let front = render_model_image(&model, 0.0, 0.0, 1.0, (96, 96));
        let turned = render_model_image(&model, 90.0, 0.0, 1.0, (96, 96));
        assert_ne!(
            front.pixels, turned.pixels,
            "换一个水平角必须画出不同的图：旋转参数没进渲染就是假预览"
        );
        // 同一组参数必须逐像素可重复（缓存与门都靠这条）。
        let again = render_model_image(&model, 0.0, 0.0, 1.0, (96, 96));
        assert_eq!(front.pixels, again.pixels, "同输入必须同输出");
    }

    #[test]
    fn zooming_in_covers_more_pixels_than_the_fit_view() {
        let model = load_model(&model_path("p3-sphere.glb")).expect("球体应能解析");
        let fit = foreground_pixels(&render_model_image(&model, 0.0, 0.0, 1.0, (128, 128)));
        let zoomed = foreground_pixels(&render_model_image(&model, 0.0, 0.0, 2.0, (128, 128)));
        assert!(
            zoomed > fit,
            "放大后占屏面积必须变大：fit={fit} zoomed={zoomed}"
        );
    }

    #[test]
    fn a_broken_glb_reports_a_reason_and_draws_nothing() {
        let root = temp_root("broken");
        let relative = "assets/models/broken.glb";
        fs::write(root.join(relative), b"not a glb").unwrap();
        let mut app = app(root.clone());
        app.selected_resource = Some(PathBuf::from(relative));
        app.resources.refresh(&root).unwrap();

        let drawn = draw_inspector(&mut app);
        assert!(
            drawn.contains("模型预览"),
            "属性面板必须显示模型段：{drawn}"
        );
        let error = app.model_preview.error.clone().expect("坏文件要有原因");
        assert!(!error.is_empty(), "原因不能是空串");
        assert!(!app.model_preview.has_texture(), "失败时不能留一张假图");
        assert_eq!(app.model_preview.stats, ModelStats::default());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn switching_models_drops_the_previous_geometry() {
        let root = temp_root("switch");
        fs::copy(
            model_path("m8-test-triangle.glb"),
            root.join("assets/models/tri.glb"),
        )
        .unwrap();
        fs::copy(
            model_path("p3-sphere.glb"),
            root.join("assets/models/sphere.glb"),
        )
        .unwrap();
        let mut app = app(root.clone());

        app.selected_resource = Some(PathBuf::from("assets/models/tri.glb"));
        app.resources.refresh(&root).unwrap();
        draw_inspector(&mut app);
        let triangles = app.model_preview.stats.triangles;
        assert!(triangles > 0);

        app.selected_resource = Some(PathBuf::from("assets/models/sphere.glb"));
        draw_inspector(&mut app);
        assert_ne!(
            app.model_preview.stats.triangles, triangles,
            "换模型后规模读数必须跟着换，不能留着上一个的几何体"
        );
        assert!(app.model_preview.has_texture());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn selecting_a_glb_draws_the_preview_inside_the_inspector() {
        let root = temp_root("inspector");
        fs::copy(
            model_path("m8-test-triangle.glb"),
            root.join("assets/models/tri.glb"),
        )
        .unwrap();
        let mut app = app(root.clone());
        app.resources.refresh(&root).unwrap();
        app.selected_resource = Some(PathBuf::from("assets/models/tri.glb"));

        let drawn = draw_inspector(&mut app);
        assert!(
            drawn.contains("模型预览"),
            "选中 glb 后属性面板必须走模型段：{drawn}"
        );
        assert!(drawn.contains("重置视角"), "视角控件必须在：{drawn}");
        assert!(
            app.model_preview.has_texture(),
            "属性面板这一帧必须真的画出并上传预览纹理"
        );
        assert_eq!(app.model_preview.error, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_preview_writes_nothing_into_the_project() {
        let root = temp_root("readonly");
        fs::copy(
            model_path("m8-test-triangle.glb"),
            root.join("assets/models/tri.glb"),
        )
        .unwrap();
        let mut app = app(root.clone());
        app.selected_resource = Some(PathBuf::from("assets/models/tri.glb"));
        app.resources.refresh(&root).unwrap();
        let before = snapshot(&root);

        draw_inspector(&mut app);
        // 拖动/缩放只改观察参数：连渲染带上传都不该在工程里留下文件。
        app.model_preview.yaw = 90.0;
        app.model_preview.scale = 2.0;
        draw_inspector(&mut app);

        assert_eq!(before, snapshot(&root), "预览不许在工程里写任何文件");
        let _ = fs::remove_dir_all(root);
    }

    fn snapshot(root: &Path) -> Vec<(PathBuf, u64)> {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push((path.clone(), entry.metadata().unwrap().len()));
                }
            }
        }
        let mut files = Vec::new();
        walk(root, &mut files);
        files.sort();
        files
    }

    #[test]
    fn clearing_the_state_forgets_the_model_and_the_angles() {
        let mut state = ModelPreviewState::new();
        state.relative = Some("assets/models/a.glb".to_owned());
        state.yaw = 120.0;
        state.error = Some("坏文件".to_owned());
        state.clear();
        assert_eq!(state.relative, None);
        assert_eq!(state.error, None);
        assert_eq!(state.yaw, DEFAULT_YAW, "清完回到默认视角");
        assert_eq!(state.pitch, DEFAULT_PITCH);
        assert_eq!(state.scale, 1.0);
    }
}
