//! GPU renderer: device/surface/pipelines, texture upload, depth/shadow
//! resources, 2D/3D render passes, and M8-08 culling/instancing. The scene
//! layer produces [`SceneMesh`]/[`Scene3d`] frames; this module consumes them.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

use crate::core::{
    asset_handle::AssetHandle,
    async_loader::{AsyncLoader, LoadMode},
    camera::{
        Camera3D, CameraView, ClearPolicy, DEFAULT_3D_CLEAR, FrameViews, RenderView, Viewport,
    },
    layer::{RenderLayer, layer_indices},
    light::Light,
    logging::{EngineError, LogModule},
    math3d::{Mat4, Vec3},
    post_process::PostProcess,
    render_plan::{FrameFacts, RenderPass, RenderPlan},
};
use wgpu::util::DeviceExt;
use wgpu::{
    CompositeAlphaMode, Device, DeviceDescriptor, Instance, InstanceDescriptor, PresentMode, Queue,
    Surface, SurfaceConfiguration, SurfaceError, TextureUsages,
};
use winit::window::Window;

use crate::host::{
    frame::{OverlayVertex, Scene3d, SceneMesh, VectorTextVertex, WorldSpriteVertex},
    shader::{
        MESH3D_SHADER, POST_PROCESS_SHADER, SHADOW_SHADER, TEXTURED_SHADER, VECTOR_TEXT_SHADER,
        WORLD_SPRITE_SHADER,
    },
};

mod arena;
mod gpu_mesh;
mod gpu_texture;
mod readback;
mod targets;
mod timestamp;
pub use readback::{ReadbackMetrics, ReadbackStatus};
use timestamp::{GpuTimestampState, TimestampSlot};

/// 需要深度附件的那几条管线（WP6）。把"要不要写深度"从散落的字段字面量收成一处**类型化**
/// 契约，是因为它曾经被脚本化编辑改错位：三个管线里有两条被误改成不写深度，而 golden 只断言
/// 可绘制对象与像素非空、**断言不了深度顺序**，于是 3D 深度静默失效。现在有单测把每条管线的
/// 深度策略逐条钉住（见 `tests::depth_and_frame_views::depth_policies_are_pinned_per_pass`）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DepthPass {
    /// 3D 网格：**写深度**，前后遮挡靠深度缓冲。
    Mesh3d,
    /// 阴影贴图：**写深度**（它就是不透明物体到光源的距离场）。
    Shadow,
    /// 世界 Sprite（**不透明**）：**写深度**。与 3D 网格共用同一份深度缓冲，并在网格 pass
    /// **之前**绘制，因此两个方向的遮挡都成立：近处的不透明精灵挡住远处的网格，近处的网格
    /// 也能挡住远处的精灵（docs/54 §8-④）。
    WorldSpriteOpaque,
    /// 世界 Sprite（**半透明**）：**测深度但不写深度**，在网格 pass 之后按画家顺序（远→近）
    /// 绘制。半透明之间因此可以正常叠加，同时仍被更深的不透明内容正确遮挡。
    WorldSprite,
}

impl DepthPass {
    /// 该管线是否写深度。
    const fn write_depth(self) -> bool {
        match self {
            Self::Mesh3d | Self::Shadow | Self::WorldSpriteOpaque => true,
            Self::WorldSprite => false,
        }
    }
}

fn depth_stencil_state(pass: DepthPass, bias: wgpu::DepthBiasState) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth32Float,
        depth_write_enabled: pass.write_depth(),
        depth_compare: wgpu::CompareFunction::LessEqual,
        stencil: wgpu::StencilState::default(),
        bias,
    }
}

/// 世界 Sprite 通道本帧用哪个视图、哪个深度附件（docs/54 §8-④）。
#[derive(Clone, Copy, Debug, PartialEq)]
struct WorldSpriteView {
    /// 世界 → 裁剪空间矩阵：共用视图时与 3D 网格**同一份**（来自宿主声明的 `RenderView`）。
    matrix: Mat4,
    /// 声明的视口；`None` = 沿用渲染器默认的 overlay 视口（宿主未声明视图的回滚档）。
    viewport: Option<Viewport>,
    /// 是否与 3D 网格共用深度附件。共用时精灵与网格互相遮挡；不共用时只有精灵之间的顺序。
    shared_depth: bool,
}

/// 一次世界 Sprite 绘制要挑哪些分段。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SpritePassFilter {
    /// 不透明（写深度）——在网格 pass 之前画。
    Opaque,
    /// 半透明（测深度不写）——在网格 pass 之后画。
    Translucent,
    /// 全部，忽略深度策略（纯 2D / 回滚档：顺序即一切）。
    All,
}

/// 世界 Sprite 通道的视图决策（纯函数，便于在没有 GPU 的环境里把契约钉死）。
///
/// 优先级见 [`Renderer::prepare_world_sprites`]：声明了主视图就与它共用相机/视口/深度；
/// 只有 overlay 时用 overlay 的世界相机但深度独立；什么都没声明才用宿主显式设置的矩阵。
fn resolve_world_sprite_view(
    main: Option<RenderView>,
    overlay: Option<RenderView>,
    main_matrix: Option<Mat4>,
    explicit: Mat4,
    default_viewport: Option<Viewport>,
) -> WorldSpriteView {
    if let Some(main) = main {
        return WorldSpriteView {
            matrix: main_matrix
                .or_else(|| main.matrix())
                .or_else(|| overlay.and_then(RenderView::matrix))
                .unwrap_or(explicit),
            viewport: Some(main.viewport),
            shared_depth: true,
        };
    }
    if let Some(overlay) = overlay {
        return WorldSpriteView {
            matrix: overlay.matrix().unwrap_or(explicit),
            viewport: Some(overlay.viewport),
            shared_depth: false,
        };
    }
    WorldSpriteView {
        matrix: explicit,
        viewport: default_viewport,
        shared_depth: false,
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// 2D 顶点缓冲初始容量（1 MiB ≈ 9700 个 36 字节顶点）。运行时按需增长，
/// 见 [`next_2d_vertex_capacity`]。
const INITIAL_2D_VERTEX_BUFFER_BYTES: u64 = 1024 * 1024;

/// 2D 顶点缓冲的目标容量：至少容纳 `required` 字节，且不小于初始容量、按 2 的幂增长。
///
/// 按 2 的幂增长是为了避免每帧为多出的几十个顶点重建 buffer（GPU 资源重建会
/// 造成帧时间尖峰），同时保证扩容次数对数级增长。
fn next_2d_vertex_capacity(required: u64) -> u64 {
    let floor = required.max(INITIAL_2D_VERTEX_BUFFER_BYTES);
    floor.checked_next_power_of_two().unwrap_or(floor)
}

/// A texture uploaded to the GPU and ready to sample. The path index lives in
/// `Renderer::texture_paths`, so the entry itself only carries the bind group.
struct LoadedTexture {
    bind_group: wgpu::BindGroup,
}

/// GPU upload state for one per-Text custom font atlas, keyed by namespaced
/// font id (`font://...`). Mirrors the default-atlas fields on `Renderer`.
struct CustomFontUpload {
    texture: Option<wgpu::Texture>,
    bind: Option<wgpu::BindGroup>,
    sdf_bind: Option<wgpu::BindGroup>,
    uploaded_version: u64,
    uploaded_hash: u64,
}

/// Shared vertex/index buffers for one mesh primitive, cached by mesh path.
#[derive(Clone)]
struct GpuGeometry {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

/// A decoded base-color texture uploaded to the GPU (sRGB) plus its view.
struct CachedAlbedoTexture {
    /// Kept alive so `view` stays valid; never read directly.
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// File-authored material parameters for one primitive. The per-draw model
/// matrix is supplied through the instance buffer instead.
#[derive(Clone, Copy)]
struct GpuMaterial {
    base_color: [f32; 4],
    metallic: f32,
    roughness: f32,
    /// Whether the owning entity has a skinned `Animation3D` palette.
    skinned: bool,
    /// Index into `Renderer::albedo_textures`; `None` samples a white fallback.
    albedo: Option<u32>,
}

/// Cached GPU geometry + file materials for one mesh path.
struct CachedMesh {
    primitives: Vec<CachedPrimitive>,
}

impl GpuMaterial {
    /// docs/54 §4.3：GPU 材质由统一的 [`crate::core::renderable::Material`] 构造——金属度/粗糙度/
    /// 基色的默认值从此只有一处，精灵与网格不会各自演化出一套"缺省材质"。
    fn from_material(
        material: &crate::core::renderable::Material,
        skinned: bool,
        albedo: Option<u32>,
    ) -> Self {
        Self {
            base_color: material.base_color,
            metallic: material.metallic,
            roughness: material.roughness,
            skinned,
            albedo,
        }
    }
}

struct CachedPrimitive {
    geometry: GpuGeometry,
    /// docs/54 §4.3：网格的材质与精灵/Quad 共用同一个 `Material` 类型（"材质是什么"只有一处
    /// 定义）；`albedo` 是它的贴图在 GPU 底图表里的下标，`None` = 白底。
    material: crate::core::renderable::Material,
    /// Index into `Renderer::albedo_textures`; `None` samples a white fallback.
    albedo: Option<u32>,
    /// Local-space bounding sphere (center, radius) used for frustum culling.
    bounds: (Vec3, f32),
}
/// A fully-resolved frame draw: cached geometry plus per-entity material, the
/// joint palette (empty for static meshes), and per-instance model matrices.
struct FrameDraw {
    geometry: GpuGeometry,
    material: GpuMaterial,
    palette: Vec<Mat4>,
    models: Vec<Mat4>,
}

/// Static drawables sharing one mesh path + primitive, accumulated for a
/// single instanced draw call.
struct StaticGroup {
    geometry: GpuGeometry,
    material: GpuMaterial,
    models: Vec<Mat4>,
}

/// 一个可复用的非阻塞回读槽：GPU 拷贝目标 buffer + 提交序号 + 映射完成标志。
///
/// M11-03：`map_result` 记录 `map_async` 的结果（0=未就绪 / 1=映射成功 / 2=映射失败），
/// 失败的映射不再被当作"已就绪"返回脏数据（R-04），而是上报错误并让槽可重试。
struct ReadbackSlot {
    buffer: wgpu::Buffer,
    submission: Option<wgpu::SubmissionIndex>,
    /// `map_async` 回调完成标志：0=未就绪，1=映射成功，2=映射失败。
    map_result: std::sync::Arc<std::sync::atomic::AtomicU8>,
    /// `map_async` 失败时的错误消息（仅 `map_result == 2` 时有意义）。
    map_error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    width: u32,
    height: u32,
    row_bytes: u32,
    bytes_per_row: u32,
    /// Reusable compact CPU output storage retained by the slot.
    pixels: Vec<u8>,
    /// Monotonic slot generation incremented when its staging buffer is rebuilt.
    generation: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderFrameMetrics {
    pub cpu_frame_us: u64,
    pub gpu_frame_us: Option<u64>,
    pub draw_calls: u64,
    pub instances: u64,
    pub triangles: u64,
}

fn record_draw_call(metrics: &mut RenderFrameMetrics) {
    metrics.draw_calls += 1;
}

/// 本帧某一层有多少段几何（`RenderPlan` 的输入事实之一）。
fn count_layer_draws(mesh: &SceneMesh, layer: RenderLayer) -> usize {
    mesh.draws
        .iter()
        .filter(|draw| draw.render_layer == layer)
        .count()
}

fn record_triangle_draw(metrics: &mut RenderFrameMetrics, vertex_count: u32) {
    record_draw_call(metrics);
    metrics.triangles += u64::from(vertex_count) / 3;
}

fn record_vertex_draw(metrics: &mut RenderFrameMetrics, vertex_count: u32) {
    record_draw_call(metrics);
    metrics.instances += u64::from(vertex_count) / 6;
    metrics.triangles += u64::from(vertex_count) / 3;
}

fn record_indexed_draw(
    metrics: &mut RenderFrameMetrics,
    index_count: u32,
    instance_count: usize,
    pass_count: u64,
) {
    metrics.draw_calls += pass_count;
    metrics.instances += instance_count as u64;
    metrics.triangles += u64::from(index_count) / 3 * instance_count as u64;
}

/// 把声明的 [`Viewport`]（浮点、世界无关）夹成目标纹理内的整数矩形。
///
/// [`Viewport`] 只描述尺寸，因此矩形左上角恒为 `(0, 0)`——渲染器的每张视图本来就渲染进
/// 自己的附件（窗口或编辑器离屏纹理），不需要在附件内部再偏移。结果**至少 1×1** 且永远
/// 落在 `target` 之内：越界的 `set_viewport` 会被 wgpu 校验直接判错，而宿主给的是逻辑
/// 像素，可能与实际附件尺寸有取整差。
fn clamp_viewport(viewport: Viewport, target: (u32, u32)) -> (u32, u32, u32, u32) {
    let target_width = target.0.max(1);
    let target_height = target.1.max(1);
    let width = (viewport.width.round().max(1.0) as u32)
        .min(target_width)
        .max(1);
    let height = (viewport.height.round().max(1.0) as u32)
        .min(target_height)
        .max(1);
    (0, 0, width, height)
}

/// 在 pass 上应用声明的视口。
///
/// 声明的视口等于整个目标时**不调用** `set_viewport`：wgpu 的默认视口就是整目标，
/// 因此"没声明"与"声明整目标"两条路径产生完全相同的命令流——收敛不引入像素差异。
fn apply_declared_viewport(
    pass: &mut wgpu::RenderPass<'_>,
    viewport: Option<Viewport>,
    target: (u32, u32),
) {
    let Some(viewport) = viewport else {
        return;
    };
    let (x, y, width, height) = clamp_viewport(viewport, target);
    if x == 0 && y == 0 && width == target.0.max(1) && height == target.1.max(1) {
        return;
    }
    pass.set_viewport(x as f32, y as f32, width as f32, height as f32, 0.0, 1.0);
}

/// Largest axis scale of a model matrix (used to inflate culling spheres).
fn max_model_scale(model: Mat4) -> f32 {
    let sx =
        model.columns[0][0].powi(2) + model.columns[0][1].powi(2) + model.columns[0][2].powi(2);
    let sy =
        model.columns[1][0].powi(2) + model.columns[1][1].powi(2) + model.columns[1][2].powi(2);
    let sz =
        model.columns[2][0].powi(2) + model.columns[2][1].powi(2) + model.columns[2][2].powi(2);
    sx.max(sy).max(sz).sqrt().max(f32::EPSILON)
}
pub struct Renderer {
    /// Present surface in window mode; `None` in offscreen (embedded) mode.
    surface: Option<Surface<'static>>,
    device: Device,
    queue: Queue,
    pub config: SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    camera_bind_group: wgpu::BindGroup,
    shadow_bind_group: wgpu::BindGroup,
    shadow_pipeline: wgpu::RenderPipeline,
    depth_view: wgpu::TextureView,
    /// Current depth-view size; rebuilt when the render target changes size.
    depth_size: (u32, u32),
    shadow_view: wgpu::TextureView,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    /// 2D 顶点缓冲扩容次数（诊断/验收用；正常游戏内应保持很小的值）。
    vertex_buffer_growths: u64,
    /// 单帧顶点字节数峰值（诊断用：容量是否接近上限）。
    vertex_bytes_peak: u64,
    textures: Vec<LoadedTexture>,
    /// P0-③：纹理路径 → `textures` 下标。`textures` 只追加不删除，因此索引恒有效，
    /// 按路径查找从 O(n) 线性扫描变为 O(1)。
    texture_paths: HashMap<String, usize>,
    /// docs/54 §4.4：资源身份 → `textures` 下标。句柄由运行时保证"同路径恒同句柄"，因此这张表
    /// 一旦建立就不会指错资源；命中即 O(1) 取 bind group（此前是每 draw 一次线性路径扫描）。
    texture_slots: HashMap<AssetHandle, usize>,
    /// 身份缓存命中/未命中次数（诊断/验收用：证明身份这条路真的被走到了）。
    texture_identity_hits: u64,
    texture_identity_misses: u64,
    /// P0-②：异步资源加载（后台读盘 + CPU 解码，主线程小步 GPU 上传）。
    /// 游戏运行（`Renderer::new`）为 Async；golden / 编辑器离屏预览（`new_offscreen`）
    /// 为 Sync，保证确定性输出里绝无异步加载。
    async_loader: AsyncLoader,
    white: Option<wgpu::BindGroup>,
    atlas: Option<wgpu::BindGroup>,
    font_atlas_texture: Option<wgpu::Texture>,
    font_atlas_bind: Option<wgpu::BindGroup>,
    /// SDF text bind group (Linear sampler); used by `draw.sdf` ranges.
    font_atlas_sdf_bind: Option<wgpu::BindGroup>,
    /// Linear sampler for SDF distance fields (text only).
    sdf_sampler: wgpu::Sampler,
    /// Pipeline for SDF text draws (Linear sampling + SDF shader branch).
    pipeline_sdf: wgpu::RenderPipeline,
    vector_pipeline: wgpu::RenderPipeline,
    vector_bind_group_layout: wgpu::BindGroupLayout,
    vector_vertex_buffer: wgpu::Buffer,
    vector_glyph_buffer: wgpu::Buffer,
    vector_segment_buffer: wgpu::Buffer,
    vector_bind_group: Option<wgpu::BindGroup>,
    vector_glyph_cache: Vec<u8>,
    vector_segment_cache: Vec<u8>,
    vector_glyph_hash: u64,
    vector_segment_hash: u64,
    font_atlas_uploaded_version: u64,
    font_atlas_uploaded_hash: u64,
    /// Per-Text custom font atlases uploaded on demand (key = `font://...` id).
    custom_fonts: HashMap<String, CustomFontUpload>,
    project_root: Option<PathBuf>,
    /// Camera uniform buffer, rewritten every frame from the active camera.
    camera_buffer: wgpu::Buffer,
    /// Directional light uniform buffer, rewritten every frame.
    light_buffer: wgpu::Buffer,
    /// Light-space camera uniform buffer, rewritten every frame from the light.
    shadow_camera_buffer: wgpu::Buffer,
    /// Shared identity joint palette bound by non-skinned draws so the shader's
    /// skinning branch is a no-op without a per-draw allocation.
    identity_palette: wgpu::Buffer,
    /// P0-①：帧级环形缓冲（bump arena）。每帧 3D 开画前偏移归零，材质 uniform 与
    /// 关节调色板依次 `write_buffer` 写入对齐块；整帧 GPU buffer 分配为零（仅容量
    /// 不足时翻倍重建，旧 bind group 仍引用旧缓冲、数据已写入，依旧有效）。
    material_arena: wgpu::Buffer,
    material_arena_used: u64,
    /// P0-①：per-instance 模型矩阵的帧级环形缓冲，同上，STORAGE 用法。
    instance_arena: wgpu::Buffer,
    instance_arena_used: u64,
    uniform_align: u64,
    storage_align: u64,
    /// GPU geometry + file materials cached by mesh path.
    mesh_cache: HashMap<String, CachedMesh>,
    /// docs/54 §4.4：网格资源身份 → `mesh_cache` 的键。运行时给了句柄就按句柄查（O(1)），
    /// 并且同一网格跨帧恒同键——路径字符串重新解析、或两个不同路径互相顶替，都不会发生。
    mesh_slots: HashMap<AssetHandle, String>,
    /// 网格身份缓存命中/未命中次数（诊断/验收用）。
    mesh_identity_hits: u64,
    mesh_identity_misses: u64,
    /// Base-color textures uploaded from glTF materials (sRGB), indexed by
    /// `GpuMaterial::albedo`. Index 0 is reserved for the white fallback.
    albedo_textures: Vec<CachedAlbedoTexture>,
    /// Shared sampler for base-color textures.
    albedo_sampler: wgpu::Sampler,
    /// 1x1 white texture view bound when a material has no base-color texture.
    white_albedo_view: wgpu::TextureView,
    /// CPU frustum culling of scene drawables (M8-08).
    cull_enabled: bool,
    /// Grouping of same mesh/material draws into instanced calls (M8-08).
    instancing_enabled: bool,
    /// Frames rendered since startup, for periodic baseline logging.
    frame_count: u64,
    _material_bind_group_layout: wgpu::BindGroupLayout,
    /// 非阻塞回读槽（离屏纹理 → CPU），避免主线程阻塞等待 GPU。
    readback_slots: Vec<ReadbackSlot>,
    readback_submitted: u64,
    readback_completed: u64,
    readback_dropped: u64,
    readback_pending_count: u64,
    readback_buffer_rebuilds: u64,
    timestamp_query_set: Option<wgpu::QuerySet>,
    timestamp_slots: Vec<TimestampSlot>,
    timestamp_period_ns: f32,
    timestamp_state: GpuTimestampState,
    timestamp_error: Option<String>,
    last_frame_metrics: RenderFrameMetrics,
    /// docs/54 §4.3：上一帧的**渲染通道计划**与**实际执行的通道**（按进入顺序）。
    ///
    /// 计划的开关条件不再散在渲染循环里，而是 `RenderPlan::for_frame` 从帧内容算出；执行完
    /// 立刻与本计划核对（[`RenderPlan::mismatch`]），不一致就告警——"少画一个通道"因此不会
    /// 静默发生。
    last_frame_plan: RenderPlan,
    last_frame_passes: Vec<RenderPass>,
    /// WP5：宿主声明的本帧相机侧视图（清屏策略 + 视口）。默认全 `None` ⇒ 渲染器保持
    /// 收敛前的行为（历史默认清屏色、整目标视口），这就是这次接线可回滚的那一档。
    frame_views: FrameViews,
    /// WP6 世界 Sprite 通道：管线、相机 uniform（视图投影矩阵）、顶点缓冲与独立深度附件。
    /// `SceneMesh::world_sprites` 为空时这一整套完全不参与渲染，因此不喂数据时零成本。
    ///
    /// 两条管线只差深度策略（不透明写深度 / 半透明不写）；共用 3D 深度附件时见
    /// [`DepthPass::WorldSpriteOpaque`] 的遮挡契约。
    world_sprite_pipeline: wgpu::RenderPipeline,
    world_sprite_opaque_pipeline: wgpu::RenderPipeline,
    world_sprite_camera_buffer: wgpu::Buffer,
    world_sprite_camera_bind_group: wgpu::BindGroup,
    world_sprite_buffer: wgpu::Buffer,
    world_sprite_capacity: u64,
    /// 与 3D 网格**不共用**时的世界 Sprite 深度附件（纯 2D 画面 / 宿主未声明视图的回滚档）。
    world_sprite_depth_view: wgpu::TextureView,
    world_sprite_depth_size: (u32, u32),
    /// 世界 Sprite 的投影（宿主每帧写入；已声明主视图时它与主视图同一份矩阵）。
    world_sprite_view_projection: Mat4,
    /// docs/54 §4.3 后处理：宿主声明（`None` = 关闭，也就是回滚档）。
    post_process: Option<PostProcess>,
    post_process_pipeline: wgpu::RenderPipeline,
    post_process_layout: wgpu::BindGroupLayout,
    post_process_sampler: wgpu::Sampler,
    post_process_params: wgpu::Buffer,
    /// 场景离屏颜色纹理（打开后处理时才创建）。视图单独存一份，好让每帧"取出 → 用完放回"，
    /// 从而不必在 `render_passes` 里同时持有 `&self` 与 `&mut self`。
    post_process_texture: Option<wgpu::Texture>,
    post_process_scene_view: Option<wgpu::TextureView>,
    post_process_size: (u32, u32),
}

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
    ) -> anyhow::Result<Self> {
        pollster::block_on(Self::new_async(
            Some(window),
            None,
            project_root,
            cull_enabled,
            instancing_enabled,
        ))
    }

    /// Creates a renderer with no window surface, using the caller-provided
    /// device/queue (e.g. the editor's eframe device). Rendering goes through
    /// [`Renderer::render_into_texture`].
    pub fn new_offscreen(
        device: Device,
        queue: Queue,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
    ) -> anyhow::Result<Self> {
        pollster::block_on(Self::new_async(
            None,
            Some((device, queue)),
            project_root,
            cull_enabled,
            instancing_enabled,
        ))
    }

    async fn new_async(
        window: Option<Arc<Window>>,
        external_device: Option<(Device, Queue)>,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
    ) -> anyhow::Result<Self> {
        // P0-②：窗口渲染（游戏运行）走异步加载；离屏渲染（golden / 编辑器预览）走同步，
        // 保证确定性输出里绝无异步加载。
        let async_mode = if window.is_some() {
            LoadMode::Async
        } else {
            LoadMode::Sync
        };
        // Surface mode: create instance/adapter/device from the window.
        // Offscreen mode: use the caller-provided device/queue.
        let (device, queue, surface, format, config, adapter_name) = if let Some(window) = window {
            let size = window.inner_size();
            let instance = Instance::new(&InstanceDescriptor::default());
            let surface = instance
                .create_surface(window)
                .map_err(|error| anyhow::anyhow!("could not create surface: {error}"))?;
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                })
                .await
                .map_err(|error| {
                    anyhow::anyhow!("could not find a compatible GPU adapter: {error}")
                })?;
            let adapter_name = adapter.get_info().name;
            let timestamp_features =
                wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
            let required_features = if adapter.features().contains(timestamp_features) {
                timestamp_features
            } else {
                wgpu::Features::empty()
            };
            let (device, queue) = adapter
                .request_device(&DeviceDescriptor {
                    label: Some("rustlua-render-device"),
                    required_features,
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                })
                .await
                .map_err(|error| anyhow::anyhow!("could not create GPU device: {error}"))?;
            let capabilities = surface.get_capabilities(&adapter);
            let format = capabilities
                .formats
                .iter()
                .copied()
                .find(|format| format.is_srgb())
                .or_else(|| capabilities.formats.first().copied())
                .ok_or_else(|| anyhow::anyhow!("GPU surface reported no supported formats"))?;
            let present_mode = if capabilities.present_modes.contains(&PresentMode::Mailbox) {
                PresentMode::Mailbox
            } else {
                PresentMode::Fifo
            };
            let alpha_mode = capabilities
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(CompositeAlphaMode::Auto);
            let config = SurfaceConfiguration {
                usage: TextureUsages::RENDER_ATTACHMENT,
                format,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode,
                alpha_mode,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            };
            surface.configure(&device, &config);
            (device, queue, Some(surface), format, config, adapter_name)
        } else {
            let (device, queue) = external_device
                .ok_or_else(|| anyhow::anyhow!("offscreen renderer requires a device/queue"))?;
            let format = wgpu::TextureFormat::Rgba8UnormSrgb;
            let config = SurfaceConfiguration {
                usage: TextureUsages::RENDER_ATTACHMENT,
                format,
                width: 1280,
                height: 720,
                present_mode: PresentMode::Fifo,
                alpha_mode: CompositeAlphaMode::Auto,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            };
            (device, queue, None, format, config, "offscreen".to_owned())
        };
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-texture-bind-group-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rustlua-textured-pipeline-layout"),
            bind_group_layouts: &[&texture_bind_group_layout],
            push_constant_ranges: &[],
        });
        let camera_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-camera3d-bind-group-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(64),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(32),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(64),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                        count: None,
                    },
                ],
            });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-textured-shader"),
            source: wgpu::ShaderSource::Wgsl(TEXTURED_SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rustlua-textured-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<OverlayVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 16,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 32,
                            shader_location: 3,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rustlua-texture-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        // SDF 文本需要 Linear 采样（距离场插值）；精灵保持 Nearest。
        let sdf_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rustlua-sdf-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipeline_sdf = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rustlua-textured-sdf-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<OverlayVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 16,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 32,
                            shader_location: 3,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let vector_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-vector-text-bind-group-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let vector_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rustlua-vector-text-pipeline-layout"),
            bind_group_layouts: &[&vector_bind_group_layout],
            push_constant_ranges: &[],
        });
        let vector_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-vector-text-shader"),
            source: wgpu::ShaderSource::Wgsl(VECTOR_TEXT_SHADER.into()),
        });
        let vector_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rustlua-vector-text-pipeline"),
            layout: Some(&vector_layout),
            vertex: wgpu::VertexState {
                module: &vector_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<VectorTextVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 16,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 20,
                            shader_location: 3,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &vector_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let vector_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-vector-text-vertex-buffer"),
            size: 1024 * 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vector_glyph_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-vector-text-glyph-buffer"),
            size: 64 * 1024,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vector_segment_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-vector-text-segment-buffer"),
            size: 1024 * 1024,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-vertex-buffer"),
            size: INITIAL_2D_VERTEX_BUFFER_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let material_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-material3d-bind-group-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(32),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(
                                (crate::core::mesh3d::MAX_BONES * 64) as u64,
                            ),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(64),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        // 首帧 uniform 的初值：相机矩阵按**目标视口**算（宽高比只从视口来，拆分批 7）。
        let initial_viewport = Viewport::new(config.width as f32, config.height as f32)?;
        let camera = Camera3D::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            0.1,
            100.0,
        )?;
        let camera_matrix = camera.view_projection_matrix(initial_viewport)?;
        let light_camera =
            Camera3D::perspective(Vec3::new(4.0, 6.0, 4.0), Vec3::ZERO, 1.3, 0.1, 30.0)?;
        let light_view = light_camera.view_matrix()?;
        let light_projection = Mat4 {
            columns: [
                [0.1, 0.0, 0.0, 0.0],
                [0.0, 0.1, 0.0, 0.0],
                [0.0, 0.0, -1.0 / 29.9, 0.0],
                [0.0, 0.0, -0.1 / 29.9, 1.0],
            ],
        };
        let light_matrix = light_projection * light_view;
        let shadow_view = Self::create_shadow_view(&device, 2048);
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rustlua-shadow-comparison-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-camera3d-buffer"),
            contents: bytemuck::cast_slice(&camera_matrix.columns),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let light_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-directional-light-buffer"),
            // 与 `Light::default()` 一致：`direction` 是**光传播方向**，所以天空里的太阳 y 分量为负。
            contents: bytemuck::cast_slice(&[[0.4_f32, -0.8, 0.5, 0.0], [1.0_f32, 1.0, 1.0, 1.0]]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let shadow_camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-shadow-camera-buffer"),
            contents: bytemuck::cast_slice(&light_matrix.columns),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let identity_palette = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-identity-joint-palette"),
            contents: bytemuck::cast_slice(&identity_palette_data()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // P0-①：帧级环形缓冲的容量与对齐来自设备限制，跨后端（DX12/Vulkan/Metal/GL）都成立。
        let uniform_align = device.limits().min_uniform_buffer_offset_alignment.max(1) as u64;
        let storage_align = device.limits().min_storage_buffer_offset_alignment.max(1) as u64;
        let material_arena = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-material3d-frame-arena"),
            size: FRAME_ARENA_INITIAL_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_arena = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-instance-models-frame-arena"),
            size: FRAME_ARENA_INITIAL_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let albedo_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rustlua-albedo-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // 1x1 white texture bound when a material has no base-color texture.
        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-albedo-white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_albedo_view = white_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-camera3d-bind-group"),
            layout: &camera_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: light_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: shadow_camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&shadow_sampler),
                },
            ],
        });
        let shadow_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-shadow-bind-group-layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(64),
                    },
                    count: None,
                }],
            });
        let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-shadow-bind-group"),
            layout: &shadow_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shadow_camera_buffer.as_entire_binding(),
            }],
        });
        let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-mesh3d-shader"),
            source: wgpu::ShaderSource::Wgsl(MESH3D_SHADER.into()),
        });
        let mesh_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rustlua-mesh3d-pipeline-layout"),
            bind_group_layouts: &[&camera_bind_group_layout, &material_bind_group_layout],
            push_constant_ranges: &[],
        });
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rustlua-mesh3d-pipeline"),
            layout: Some(&mesh_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &mesh_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<crate::core::mesh3d::MeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 24,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32x4,
                            offset: 32,
                            shader_location: 3,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 48,
                            shader_location: 4,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &mesh_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(depth_stencil_state(
                DepthPass::Mesh3d,
                wgpu::DepthBiasState::default(),
            )),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-shadow-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rustlua-shadow-pipeline-layout"),
                bind_group_layouts: &[&shadow_bind_group_layout, &material_bind_group_layout],
                push_constant_ranges: &[],
            });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rustlua-shadow-pipeline"),
            layout: Some(&shadow_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<crate::core::mesh3d::MeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 24,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32x4,
                            offset: 32,
                            shader_location: 3,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 48,
                            shader_location: 4,
                        },
                    ],
                }],
            },
            fragment: None,
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(depth_stencil_state(
                DepthPass::Shadow,
                wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            )),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let depth_view = Self::create_depth_view(&device, config.width, config.height);
        let depth_init = (config.width, config.height);
        let timestamp_enabled = device.features().contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        );
        let timestamp_query_set = timestamp_enabled.then(|| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("rustlua-frame-timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            })
        });
        let timestamp_period_ns = queue.get_timestamp_period();
        // WP6 世界 Sprite 通道：独立的一套资源（2D 相机 uniform + 顶点缓冲 + 深度附件）。
        // 与 overlay 路径互不影响：宿主不喂世界顶点时它一次也不会被用到。
        let world_sprite_camera_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-world-sprite-camera-layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let world_sprite_camera_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-world-sprite-camera"),
                contents: bytemuck::bytes_of(&IDENTITY_MAT4),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let world_sprite_camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-world-sprite-camera-bind-group"),
            layout: &world_sprite_camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: world_sprite_camera_buffer.as_entire_binding(),
            }],
        });
        let world_sprite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-world-sprite-shader"),
            source: wgpu::ShaderSource::Wgsl(WORLD_SPRITE_SHADER.into()),
        });
        let world_sprite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rustlua-world-sprite-pipeline-layout"),
                bind_group_layouts: &[&world_sprite_camera_layout, &texture_bind_group_layout],
                push_constant_ranges: &[],
            });
        // 世界 Sprite 的两条管线只差深度策略：不透明写深度（可与网格互相遮挡），半透明
        // 测深度不写深度（画家顺序叠加）。除 `DepthPass` 外必须逐字段相同，否则"同一份几何
        // 换一个策略"就会变成两条不同的渲染路径。
        let create_world_sprite_pipeline = |label: &str, pass: DepthPass| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&world_sprite_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &world_sprite_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<WorldSpriteVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 12,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 20,
                                shader_location: 2,
                            },
                        ],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &world_sprite_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(depth_stencil_state(pass, wgpu::DepthBiasState::default())),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let world_sprite_pipeline =
            create_world_sprite_pipeline("rustlua-world-sprite-pipeline", DepthPass::WorldSprite);
        let world_sprite_opaque_pipeline = create_world_sprite_pipeline(
            "rustlua-world-sprite-opaque-pipeline",
            DepthPass::WorldSpriteOpaque,
        );
        let world_sprite_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-world-sprite-buffer"),
            size: WORLD_SPRITE_BUFFER_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let world_sprite_depth_view = Self::create_depth_view(&device, 1, 1);
        // docs/54 §4.3：后处理通道的管线与资源。宿主不打开时这些都只是闲置对象（不产生 pass、
        // 不产生离屏纹理），因此"没开后处理"的运行时代价与收敛前一致。
        let post_process_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-post-process-shader"),
            source: wgpu::ShaderSource::Wgsl(POST_PROCESS_SHADER.into()),
        });
        let post_process_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-post-process-bind-group-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let post_process_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rustlua-post-process-pipeline-layout"),
                bind_group_layouts: &[&post_process_layout],
                push_constant_ranges: &[],
            });
        let post_process_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rustlua-post-process-pipeline"),
                layout: Some(&post_process_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &post_process_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    // 全屏三角形由 `@builtin(vertex_index)` 生成，因此没有顶点缓冲。
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &post_process_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
        let post_process_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rustlua-post-process-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let post_process_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-post-process-params"),
            // 32 字节：exposure + vignette + 两个填充位，随后是一个 vec4 着色（见 crate::core::post_process）。
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        tracing::info!(target: "game",
            "[render] initialized: adapter={adapter_name} format={format:?} present_mode={:?} gpu_timestamp={timestamp_enabled}",
            config.present_mode
        );
        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            mesh_pipeline,
            camera_bind_group,
            shadow_bind_group,
            shadow_pipeline,
            depth_view,
            depth_size: depth_init,
            shadow_view,
            texture_bind_group_layout,
            sampler,
            vertex_buffer,
            vertex_buffer_growths: 0,
            vertex_bytes_peak: 0,
            textures: Vec::new(),
            texture_paths: HashMap::new(),
            texture_slots: HashMap::new(),
            texture_identity_hits: 0,
            texture_identity_misses: 0,
            async_loader: AsyncLoader::new(async_mode),
            white: None,
            atlas: None,
            font_atlas_texture: None,
            font_atlas_bind: None,
            font_atlas_sdf_bind: None,
            sdf_sampler,
            pipeline_sdf,
            vector_pipeline,
            vector_bind_group_layout,
            vector_vertex_buffer,
            vector_glyph_buffer,
            vector_segment_buffer,
            vector_bind_group: None,
            vector_glyph_cache: Vec::new(),
            vector_segment_cache: Vec::new(),
            vector_glyph_hash: 0,
            vector_segment_hash: 0,
            font_atlas_uploaded_version: u64::MAX,
            font_atlas_uploaded_hash: 0,
            custom_fonts: HashMap::new(),
            project_root,
            camera_buffer,
            light_buffer,
            shadow_camera_buffer,
            identity_palette,
            material_arena,
            material_arena_used: 0,
            instance_arena,
            instance_arena_used: 0,
            uniform_align,
            storage_align,
            mesh_cache: HashMap::new(),
            mesh_slots: HashMap::new(),
            mesh_identity_hits: 0,
            mesh_identity_misses: 0,
            albedo_textures: Vec::new(),
            albedo_sampler,
            white_albedo_view,
            cull_enabled,
            instancing_enabled,
            frame_count: 0,
            _material_bind_group_layout: material_bind_group_layout,
            readback_slots: Vec::new(),
            readback_submitted: 0,
            readback_completed: 0,
            readback_dropped: 0,
            readback_pending_count: 0,
            readback_buffer_rebuilds: 0,
            timestamp_query_set,
            timestamp_slots: Vec::new(),
            timestamp_period_ns,
            timestamp_state: if timestamp_enabled {
                GpuTimestampState::Ready
            } else {
                GpuTimestampState::Unsupported
            },
            timestamp_error: None,
            last_frame_metrics: RenderFrameMetrics::default(),
            last_frame_plan: RenderPlan::empty(),
            last_frame_passes: Vec::new(),
            frame_views: FrameViews::new(),
            world_sprite_pipeline,
            world_sprite_opaque_pipeline,
            world_sprite_camera_buffer,
            world_sprite_camera_bind_group,
            world_sprite_buffer,
            world_sprite_capacity: WORLD_SPRITE_BUFFER_BYTES,
            world_sprite_depth_view,
            world_sprite_depth_size: (0, 0),
            world_sprite_view_projection: Mat4 {
                columns: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
            },
            post_process: None,
            post_process_pipeline,
            post_process_layout,
            post_process_sampler,
            post_process_params,
            post_process_texture: None,
            post_process_scene_view: None,
            post_process_size: (0, 0),
        })
    }

    /// Rewrites the camera, light, and light-space uniform buffers from the
    /// active scene's camera and directional light.
    ///
    /// `view_projection` 是宿主声明的世界视图矩阵（docs/54 §8-③）。有声明就用声明的那一份，
    /// 没有才回退到 `Scene3d.view`——那是**同一次视口烘焙**的结果（相机拆分批 7），因此回退
    /// 路径与声明路径不可能给出两个不同的比例。
    fn update_3d_uniforms(&mut self, scene: &Scene3d, view_projection: Option<Mat4>) {
        let Some(matrix) = view_projection.or_else(|| scene.view.matrix()) else {
            // 宿主既没声明视图、相机又退化：不写 uniform（保持上一帧的值），并留下诊断。
            // 这条路径不该在正常帧里出现——世界视图矩阵的唯一来源是宿主声明的 `RenderView`。
            tracing::warn!(
                target: "render",
                "[render] 世界视图矩阵缺失：宿主未声明 RenderView 且场景相机退化，3D pass 使用上一帧矩阵"
            );
            self.write_light_uniforms(scene);
            return;
        };
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&matrix.columns),
        );
        if view_projection.is_none() {
            // 合法回滚档：宿主没声明视图时回退到场景提取时烘焙的视图。trace 级别，不刷屏。
            tracing::trace!(target: "render", "[render] 3D view matrix fell back to Scene3d.view");
        }
        self.write_light_uniforms(scene);
    }

    /// 光照与阴影 uniform（与相机矩阵分开写，缺相机时也要更新）。
    fn write_light_uniforms(&mut self, scene: &Scene3d) {
        let (direction, color_intensity) = scene.light.uniform();
        self.queue.write_buffer(
            &self.light_buffer,
            0,
            bytemuck::cast_slice(&[direction, color_intensity]),
        );
        let light_matrix = light_view_projection(scene.light);
        self.queue.write_buffer(
            &self.shadow_camera_buffer,
            0,
            bytemuck::cast_slice(&light_matrix.columns),
        );
    }

    /// 决定本帧世界 Sprite 用哪个视图/深度附件，并把顶点与相机 uniform 准备好。
    ///
    /// 视图来源优先级（docs/54 §8-③/④）：
    /// 1. 宿主声明了主视图 ⇒ 与主视图**共用**相机、视口与深度附件（混合 2D/3D 的正确形态）；
    /// 2. 只有 overlay（纯 2D 画面）⇒ 用 overlay 的世界相机，深度附件独立；
    /// 3. 什么都没声明 ⇒ 用宿主显式设置的矩阵与默认 overlay 视口（收敛前的回滚档）。
    fn prepare_world_sprites(
        &mut self,
        mesh: &SceneMesh,
        main_matrix: Option<Mat4>,
        overlay_viewport: Option<Viewport>,
        target: (u32, u32),
    ) -> Option<WorldSpriteView> {
        let bytes = bytemuck::cast_slice(&mesh.world_sprites);
        self.ensure_world_sprite_capacity(bytes.len() as u64);
        self.queue.write_buffer(&self.world_sprite_buffer, 0, bytes);
        let view = resolve_world_sprite_view(
            self.frame_views.main,
            self.frame_views.overlay,
            main_matrix,
            self.world_sprite_view_projection,
            overlay_viewport,
        );
        // 深度附件独立时按目标尺寸准备（共用时用 3D 那份，尺寸由 create_depth_view 维护）。
        if !view.shared_depth && self.world_sprite_depth_size != (target.0.max(1), target.1.max(1))
        {
            self.world_sprite_depth_view =
                Self::create_depth_view(&self.device, target.0.max(1), target.1.max(1));
            self.world_sprite_depth_size = (target.0.max(1), target.1.max(1));
        }
        let uniform = bytemuck::cast_slice(&view.matrix.columns);
        self.queue
            .write_buffer(&self.world_sprite_camera_buffer, 0, uniform);
        // 共用视图时把同一份矩阵也记进宿主字段：诊断读到的永远是"这一帧真正用的那一份"。
        self.world_sprite_view_projection = view.matrix;
        Some(view)
    }

    /// 按 [`SpritePassFilter`] 画世界 Sprite 分段。
    #[allow(clippy::too_many_arguments)]
    fn draw_world_sprites(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target: (u32, u32),
        world: WorldSpriteView,
        pass_kind: DepthPass,
        draws: &[crate::core::sprite_batch::SpriteDrawBatch],
        paths: &[String],
        handles: &[AssetHandle],
        filter: SpritePassFilter,
        clear_depth: bool,
        metrics: &mut RenderFrameMetrics,
    ) {
        let selected: Vec<_> = draws
            .iter()
            .filter(|draw| match filter {
                SpritePassFilter::Opaque => draw.depth_write,
                SpritePassFilter::Translucent => !draw.depth_write,
                SpritePassFilter::All => true,
            })
            .collect();
        if selected.is_empty() {
            return;
        }
        let depth_view = if world.shared_depth {
            &self.depth_view
        } else {
            &self.world_sprite_depth_view
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(if world.shared_depth {
                "rustlua-world-sprite-pass"
            } else {
                "rustlua-world-sprite-pass-isolated"
            }),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: if clear_depth {
                        wgpu::LoadOp::Clear(1.0)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(match pass_kind {
            DepthPass::WorldSpriteOpaque => &self.world_sprite_opaque_pipeline,
            _ => &self.world_sprite_pipeline,
        });
        pass.set_bind_group(0, &self.world_sprite_camera_bind_group, &[]);
        pass.set_vertex_buffer(0, self.world_sprite_buffer.slice(..));
        apply_declared_viewport(&mut pass, world.viewport, target);
        for draw in selected {
            let texture = draw.texture_id + 1;
            let Some(bind) = self.bind_group_for(texture, paths, handles) else {
                continue;
            };
            pass.set_bind_group(1, &bind, &[]);
            pass.draw(
                draw.vertex_start as u32..(draw.vertex_start + draw.vertex_count) as u32,
                0..1,
            );
            record_vertex_draw(metrics, draw.vertex_count as u32);
        }
    }

    /// Gathers this frame's 3D draws: every scene entity with both a
    /// `Transform3D` and a `MeshRenderer`, CPU-culled against the camera
    /// frustum, resolved to cached geometry. Static drawables sharing a mesh
    /// path and primitive are grouped into one instanced draw when enabled.
    fn frame_3d_draws(&mut self, scene: &Scene3d) -> Vec<FrameDraw> {
        // 剔除视锥来自本帧**视图的烘焙结果**（拆分批 7）：与写进 uniform 的矩阵同源，
        // 因此"画出来的"与"被剔除的"不可能按两个比例各算一套。
        let frustum = if self.cull_enabled {
            scene.view.baked().map(CameraView::frustum)
        } else {
            None
        };
        let mut groups: HashMap<(String, usize), StaticGroup> = HashMap::new();
        let mut draws: Vec<FrameDraw> = Vec::new();
        let instancing_enabled = self.instancing_enabled;
        for drawable in &scene.drawables {
            let Some(cached) = self.cached_mesh(drawable.mesh_handle, &drawable.mesh_path) else {
                continue;
            };
            for (prim_index, primitive) in cached.primitives.iter().enumerate() {
                if let Some(frustum) = frustum {
                    let world_center = drawable
                        .model
                        .transform_point(primitive.bounds.0)
                        .unwrap_or(primitive.bounds.0);
                    let world_radius = primitive.bounds.1 * max_model_scale(drawable.model);
                    if !frustum.contains_sphere(world_center, world_radius) {
                        continue;
                    }
                }
                let material = GpuMaterial::from_material(
                    &primitive.material,
                    !drawable.palette.is_empty(),
                    primitive.albedo,
                );
                if drawable.palette.is_empty() && instancing_enabled {
                    groups
                        .entry((drawable.mesh_path.clone(), prim_index))
                        .or_insert_with(|| StaticGroup {
                            geometry: primitive.geometry.clone(),
                            material,
                            models: Vec::new(),
                        })
                        .models
                        .push(drawable.model);
                } else {
                    draws.push(FrameDraw {
                        geometry: primitive.geometry.clone(),
                        material,
                        palette: drawable.palette.clone(),
                        models: vec![drawable.model],
                    });
                }
            }
        }
        for group in groups.into_values() {
            draws.push(FrameDraw {
                geometry: group.geometry,
                material: group.material,
                palette: Vec::new(),
                models: group.models,
            });
        }
        // Periodic M8-08 baseline: instance/draw counts with culling and
        // instancing active, useful for comparing against --no-cull/--no-instance.
        self.frame_count = self.frame_count.wrapping_add(1);
        if self.frame_count.is_multiple_of(120) {
            let instances: usize = draws.iter().map(|draw| draw.models.len()).sum();
            tracing::info!(
                target: "render",
                draws = draws.len(),
                instances,
                cull = self.cull_enabled,
                instancing = self.instancing_enabled,
                "[render] 3D baseline draws/instances"
            );
        }
        draws
    }

    /// 执行后处理通道：把离屏场景颜色按曝光/色调/暗角写进目标附件。
    fn draw_post_process(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
        settings: PostProcess,
        target_size: (u32, u32),
    ) -> u64 {
        self.queue.write_buffer(
            &self.post_process_params,
            0,
            bytemuck::bytes_of(&settings.uniform()),
        );
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-post-process-bind-group"),
            layout: &self.post_process_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.post_process_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.post_process_params.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rustlua-post-process-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // 全屏三角形覆盖整个附件，但 Load 会保留附件上原有内容；显式 Clear 让结果
                    // 与"上一帧残留"无关（同一张附件被复用时才看得出来）。
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.post_process_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        // 整目标：场景离屏纹理与目标同尺寸，而场景本身的视口只影响它写进离屏的那部分。
        apply_declared_viewport(&mut pass, None, target_size);
        pass.draw(0..3, 0..1);
        3
    }

    /// WP5：声明本帧的相机侧视图（清屏策略 + 视口）。
    ///
    /// 收敛前渲染器把清屏色**写死在 pass 里**、视口恒为整个目标，宿主想说"这块区域
    /// 只画 2D overlay"或"这帧不要清屏"都无处可说。现在这两件事都由 [`crate::core::camera::FrameViews`]
    /// 声明，渲染器只消费不再自己决定。
    ///
    /// 回滚档：**不调用**本函数（或传 [`FrameViews::new`]）即回到收敛前的行为——
    /// 历史默认清屏色 + 整目标视口，像素逐字节相同。
    pub fn set_frame_views(&mut self, views: FrameViews) {
        self.frame_views = views;
    }

    /// 当前生效的视图声明（诊断/验收用）。
    #[must_use]
    pub fn frame_views(&self) -> FrameViews {
        self.frame_views
    }

    /// docs/54 §4.3：打开 / 关闭后处理通道。
    ///
    /// - `Some(settings)`：场景先画进离屏颜色纹理，再由全屏 pass 施加曝光/色调/暗角写进目标附件。
    /// - `None`（默认，也是**回滚档**）：这一套完全不存在——不创建离屏纹理、不产生额外 pass，
    ///   像素与收敛前逐字节相同。
    ///
    /// 参数非法（非有限、负曝光/负色调、暗角越界）由 [`PostProcess::new`] 拒绝；本函数接受的是
    /// 已经校验过的值，因此不会在渲染中途失败。
    pub fn set_post_process(&mut self, settings: Option<PostProcess>) {
        self.post_process = settings;
    }

    /// 当前后处理设置（诊断/验收用）。
    #[must_use]
    pub fn post_process(&self) -> Option<PostProcess> {
        self.post_process
    }

    /// WP6：写入世界 Sprite 通道用的投影矩阵——**只在宿主没有声明任何 [`crate::core::camera::FrameViews`] 时**
    /// 才需要（纯 2D 回滚档）。声明了主视图或 overlay 时，世界 Sprite 用声明里的那一份矩阵
    /// （与网格同一份，见 docs/54 §8-③/④），本函数写入的值会被当帧覆盖。
    pub fn set_world_sprite_camera(&mut self, view_projection: Mat4) {
        self.world_sprite_view_projection = view_projection;
        // uniform 存的是列主序 4×4（与 3D 相机 uniform 同一布局）。
        let uniform: [f32; 16] = bytemuck::cast(view_projection.columns);
        self.queue.write_buffer(
            &self.world_sprite_camera_buffer,
            0,
            bytemuck::bytes_of(&uniform),
        );
    }

    /// 确保世界 Sprite 顶点缓冲能容纳 `required` 字节（按 2 的幂增长，与 2D overlay 同思路）。
    fn ensure_world_sprite_capacity(&mut self, required: u64) -> u64 {
        if required <= self.world_sprite_buffer.size() {
            return self.world_sprite_buffer.size();
        }
        let mut capacity = self
            .world_sprite_buffer
            .size()
            .max(WORLD_SPRITE_BUFFER_BYTES);
        while capacity < required {
            capacity *= 2;
        }
        self.world_sprite_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-world-sprite-buffer"),
            size: capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.world_sprite_capacity = capacity;
        capacity
    }

    pub fn render(&mut self, mesh: &SceneMesh, scene3d: Option<&Scene3d>) -> RenderOutcome {
        let Some(surface) = &self.surface else {
            tracing::warn!(target: "game", "[render] render() requires a window surface; use render_into_texture");
            return RenderOutcome::Skipped;
        };
        let output = match surface.get_current_texture() {
            Ok(output) => output,
            Err(SurfaceError::Lost | SurfaceError::Outdated) => {
                surface.configure(&self.device, &self.config);
                tracing::info!(target: "game", "[render] surface reconfigured after transient error");
                return RenderOutcome::Retry;
            }
            Err(SurfaceError::Timeout) => {
                tracing::info!(target: "game", "[render] surface acquire timed out; skipping frame");
                return RenderOutcome::Skipped;
            }
            Err(SurfaceError::OutOfMemory) => {
                tracing::error!(target: "engine", error = %EngineError::new(LogModule::Engine, "render frame", "GPU is out of memory").with_suggestion("close other GPU applications or reduce resource usage"), "GPU out of memory; requesting shutdown");
                return RenderOutcome::Fatal;
            }
            Err(SurfaceError::Other) => {
                tracing::info!(target: "game", "[render] surface acquire failed with an unknown error");
                return RenderOutcome::Skipped;
            }
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-clear-encoder"),
            });
        let started = Instant::now();
        let completed_gpu_us = self.poll_gpu_timestamp();
        let timestamp_slot = self.acquire_timestamp_slot();
        if timestamp_slot.is_some()
            && let Some(query_set) = &self.timestamp_query_set
        {
            encoder.write_timestamp(query_set, 0);
        }
        let mut metrics = self.render_passes(
            &mut encoder,
            mesh,
            scene3d,
            &view,
            (self.config.width, self.config.height),
        );
        if let Some(slot) = timestamp_slot {
            self.finish_gpu_timestamp(&mut encoder, slot);
        }
        let submission = self.queue.submit([encoder.finish()]);
        if let Some(slot) = timestamp_slot {
            self.arm_timestamp_slot(slot, submission);
        }
        metrics.cpu_frame_us = started.elapsed().as_micros() as u64;
        metrics.gpu_frame_us = completed_gpu_us;
        self.last_frame_metrics = metrics;
        output.present();
        RenderOutcome::Presented
    }

    /// Renders the current frame into an offscreen texture view (no present).
    /// The caller owns the target texture and supplies its view + dimensions;
    /// the depth attachment is rebuilt to match the target size.
    pub fn render_into_texture(
        &mut self,
        mesh: &SceneMesh,
        scene3d: Option<&Scene3d>,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> RenderOutcome {
        self.ensure_depth_view(width, height);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-offscreen-encoder"),
            });
        let started = Instant::now();
        let completed_gpu_us = self.poll_gpu_timestamp();
        let timestamp_slot = self.acquire_timestamp_slot();
        if timestamp_slot.is_some()
            && let Some(query_set) = &self.timestamp_query_set
        {
            encoder.write_timestamp(query_set, 0);
        }
        let mut metrics = self.render_passes(&mut encoder, mesh, scene3d, view, (width, height));
        if let Some(slot) = timestamp_slot {
            self.finish_gpu_timestamp(&mut encoder, slot);
        }
        let submission = self.queue.submit([encoder.finish()]);
        if let Some(slot) = timestamp_slot {
            self.arm_timestamp_slot(slot, submission);
        }
        metrics.cpu_frame_us = started.elapsed().as_micros() as u64;
        metrics.gpu_frame_us = completed_gpu_us;
        self.last_frame_metrics = metrics;
        RenderOutcome::Presented
    }

    /// Shared pass rendering into a given color view (window surface or
    /// offscreen target): clears the view, draws 3D (shadow + mesh into the
    /// depth view), then the 2D overlay.
    ///
    /// WP5：清屏与视口不再由本函数写死——它们来自宿主声明的 [`crate::core::camera::FrameViews`]。
    fn render_passes(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        mesh: &SceneMesh,
        scene3d: Option<&Scene3d>,
        view: &wgpu::TextureView,
        target: (u32, u32),
    ) -> RenderFrameMetrics {
        // 清屏：宿主声明了主视图就按声明走（`ClearPolicy::None` = 不清，叠在已有内容上）；
        // 没声明则保持收敛前的历史默认色，因此"不调用 set_frame_views"像素完全不变。
        let clear = match self.frame_views.clear() {
            Some(ClearPolicy::Color(color)) => Some(wgpu::Color {
                r: f64::from(color[0]),
                g: f64::from(color[1]),
                b: f64::from(color[2]),
                a: f64::from(color[3]),
            }),
            Some(ClearPolicy::None) => None,
            None => Some(wgpu::Color {
                r: f64::from(DEFAULT_3D_CLEAR[0]),
                g: f64::from(DEFAULT_3D_CLEAR[1]),
                b: f64::from(DEFAULT_3D_CLEAR[2]),
                a: f64::from(DEFAULT_3D_CLEAR[3]),
            }),
        };
        let main_viewport = self.frame_views.main.map(|declared| declared.viewport);
        let overlay_viewport = self.frame_views.overlay.map(|declared| declared.viewport);
        // docs/54 §4.3：打开后处理后场景先画进**离屏**附件，那张附件上没有"上一帧留下的内容"
        // 可叠，因此 `ClearPolicy::None` 无法照原义执行。这里显式改成历史默认清屏色并告警一次
        // ——静默把 `None` 当 `Color(默认)` 会让"我声明了不清屏却没生效"变成无从查起的问题。
        let clear = match clear {
            None if self.post_process.is_some() => {
                tracing::warn!(
                    target: "render",
                    "[runtime] post-process需要确定的基色：本帧的 ClearPolicy::None 按默认清屏色执行"
                );
                Some(wgpu::Color {
                    r: f64::from(DEFAULT_3D_CLEAR[0]),
                    g: f64::from(DEFAULT_3D_CLEAR[1]),
                    b: f64::from(DEFAULT_3D_CLEAR[2]),
                    a: f64::from(DEFAULT_3D_CLEAR[3]),
                })
            }
            clear => clear,
        };
        // docs/54 §8-③：本帧世界视图的投影矩阵**只有一份来源**——宿主声明的 `RenderView`。
        // 渲染器不再自己回查 `Camera3D`（那只是"宿主没声明"时的历史回退），回退吃的是
        // 场景提取时就已按视口烘焙好的 `Scene3d.view`（拆分批 7）。
        let main_matrix = self
            .frame_views
            .main
            .and_then(RenderView::matrix)
            .or_else(|| scene3d.and_then(|scene| scene.view.matrix()));
        let frame_draws = if let Some(scene) = scene3d {
            self.update_3d_uniforms(scene, main_matrix);
            self.frame_3d_draws(scene)
        } else {
            Vec::new()
        };
        let mut metrics = RenderFrameMetrics::default();
        for draw in &frame_draws {
            record_indexed_draw(
                &mut metrics,
                draw.geometry.index_count,
                draw.models.len(),
                2,
            );
        }
        // 世界 Sprite 通道（docs/54 §8-④）：与网格**共用相机、视口与深度附件**。
        // 这一段只做 CPU 侧准备（uniform + 缓冲容量），不碰 encoder，因此可以排在清屏之前，
        // 好让下面的通道计划拿到 `shared_depth` 这个事实。
        let world_view = if mesh.world_sprites.is_empty() {
            None
        } else {
            self.prepare_world_sprites(mesh, main_matrix, overlay_viewport, target)
        };
        // docs/54 §4.3：打开后处理时本帧所有场景通道都画进**离屏场景颜色纹理**，最后由
        // `PostProcess` 通道采样它写进目标附件。视图在这里"取出"、渲染完放回——这样渲染循环里
        // 不必同时持有 `&self`（画目标）与 `&mut self`（各通道的准备与状态）。
        let post_process = self.post_process;
        let scene_view = if post_process.is_some() {
            self.ensure_post_process_scene_view(target.0, target.1);
            self.post_process_scene_view.take()
        } else {
            None
        };
        let draw_target: &wgpu::TextureView = scene_view.as_ref().unwrap_or(view);
        // docs/54 §4.3：本帧跑哪些通道由**计划**决定，而不是散在下面的 `if` 里。
        // 计划是纯函数（`RenderPlan::for_frame`）⇒ 顺序与取舍可以在不启动 GPU 的单测里断言。
        let plan = RenderPlan::for_frame(FrameFacts {
            clear: clear.is_some(),
            has_3d_draws: !frame_draws.is_empty(),
            world_sprites: mesh.world_sprites.len(),
            world_sprite_opaque_segment: mesh
                .world_sprite_draws
                .iter()
                .any(|draw| draw.depth_write),
            world_sprite_shared_depth: world_view.is_some_and(|world| world.shared_depth),
            world_2d_draws: count_layer_draws(mesh, RenderLayer::World2D),
            overlay_2d_draws: count_layer_draws(mesh, RenderLayer::Overlay2D),
            screen_ui_draws: count_layer_draws(mesh, RenderLayer::ScreenUI),
            vector_text: !mesh.vector_vertices.is_empty(),
            post_process: post_process.is_some(),
        });
        let mut executed: Vec<RenderPass> = Vec::with_capacity(plan.len());
        if plan.contains(RenderPass::Clear)
            && let Some(clear) = clear
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: draw_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            executed.push(RenderPass::Clear);
        }
        // 深度附件是否已被本帧某个 pass 清过。共用深度时"谁先画谁清"、后面的 pass 只 Load；
        // 否则精灵与网格的深度就不是同一个空间里的值。
        let mut depth_cleared = false;
        // ① 不透明世界 Sprite：在网格 pass **之前**写深度 ⇒ 不透明精灵可以遮挡网格。
        if plan.contains(RenderPass::WorldSpriteOpaque)
            && let Some(world) = world_view.filter(|world| world.shared_depth)
        {
            self.draw_world_sprites(
                encoder,
                draw_target,
                target,
                world,
                DepthPass::WorldSpriteOpaque,
                &mesh.world_sprite_draws,
                &mesh.paths,
                &mesh.path_handles,
                SpritePassFilter::Opaque,
                true,
                &mut metrics,
            );
            depth_cleared = true;
            executed.push(RenderPass::WorldSpriteOpaque);
        }
        // P0-①：本帧的材质 bind group 在进入任何 pass 前一次性建好（阴影与网格共用同一份），
        // 环形缓冲先复位再填。此前是"每 Draw Call 每 Pass"分配 GPU buffer + bind group。
        let material_binds: Vec<wgpu::BindGroup> = if !frame_draws.is_empty()
            && (plan.contains(RenderPass::Shadow) || plan.contains(RenderPass::DepthOpaque))
        {
            self.material_arena_used = 0;
            self.instance_arena_used = 0;
            frame_draws
                .iter()
                .map(|draw| self.material_bind_group(draw.material, &draw.palette, &draw.models))
                .collect()
        } else {
            Vec::new()
        };
        if plan.contains(RenderPass::Shadow) {
            let mut shadow_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-shadow-pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            shadow_pass.set_pipeline(&self.shadow_pipeline);
            shadow_pass.set_bind_group(0, &self.shadow_bind_group, &[]);
            for (index, draw) in frame_draws.iter().enumerate() {
                shadow_pass.set_bind_group(1, &material_binds[index], &[]);
                shadow_pass.set_vertex_buffer(0, draw.geometry.vertex_buffer.slice(..));
                shadow_pass.set_index_buffer(
                    draw.geometry.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                shadow_pass.draw_indexed(
                    0..draw.geometry.index_count,
                    0,
                    0..draw.models.len() as u32,
                );
            }
            executed.push(RenderPass::Shadow);
        }
        if plan.contains(RenderPass::DepthOpaque) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-mesh3d-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: draw_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        // 不透明世界 Sprite 已经清过并写过这份深度；此时只 Load，网格与精灵
                        // 才在同一个深度空间里互相遮挡。
                        load: if depth_cleared {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(1.0)
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            depth_cleared = true;
            pass.set_pipeline(&self.mesh_pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            apply_declared_viewport(&mut pass, main_viewport, target);
            for (index, draw) in frame_draws.iter().enumerate() {
                pass.set_bind_group(1, &material_binds[index], &[]);
                pass.set_vertex_buffer(0, draw.geometry.vertex_buffer.slice(..));
                pass.set_index_buffer(
                    draw.geometry.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                pass.draw_indexed(0..draw.geometry.index_count, 0, 0..draw.models.len() as u32);
            }
            executed.push(RenderPass::DepthOpaque);
        }
        if plan.contains(RenderPass::WorldSprite)
            && let Some(world) = world_view
        {
            // ② 共用视图：半透明精灵在网格之后按画家顺序画，测深度但不写深度。
            // ③ 纯 2D / 宿主未声明视图（回滚档）：只有精灵之间的顺序，深度策略不参与，
            //    因此整段照旧按宿主排好的顺序画。
            let filter = if world.shared_depth {
                SpritePassFilter::Translucent
            } else {
                SpritePassFilter::All
            };
            self.draw_world_sprites(
                encoder,
                draw_target,
                target,
                world,
                DepthPass::WorldSprite,
                &mesh.world_sprite_draws,
                &mesh.paths,
                &mesh.path_handles,
                filter,
                !depth_cleared,
                &mut metrics,
            );
            executed.push(RenderPass::WorldSprite);
        }
        if plan.contains(RenderPass::Legacy2DOverlay) || plan.contains(RenderPass::ScreenUI) {
            let bytes = bytemuck::cast_slice(&mesh.vertices);
            // 2D 顶点缓冲按需增长。此前是固定 1 MiB + `if bytes.len() <= size`
            // **没有 else 分支**：超限时整个 2D 层（含 UI/文字）静默消失，日志里
            // 什么线索都没有。现在超限即扩容并留下可诊断的计数与日志。
            self.ensure_2d_vertex_capacity(bytes.len() as u64);
            self.queue.write_buffer(&self.vertex_buffer, 0, bytes);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-scene-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: draw_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            apply_declared_viewport(&mut pass, overlay_viewport, target);
            if let Some(font) = &mesh.font_atlas {
                let _ = self.ensure_font_atlas(font);
            }
            for font in &mesh.custom_font_atlases {
                let _ = self.ensure_custom_font(&font.key, &font.pixels);
            }
            // docs/54 §8-⑤：2D 叠加通道按 `RenderLayer` **分桶**后绘制
            // （World2D → Overlay2D → ScreenUI）。宿主本来就按这个顺序产出顶点，因此像素与
            // "照缓冲顺序画"逐字节一致；但层的先后从此由渲染器保证——HUD 不会再被一段插到它
            // 后面的调试线盖住，而这类错序类型检查抓不到。
            //
            // docs/54 §4.3：`Legacy2DOverlay`（World2D + Overlay2D 两个桶）与 `ScreenUI` 是计划里
            // 两条**独立通道**，因此这里按通道分桶；两者共用一次 `begin_render_pass`（同一条管线，
            // 拆成两个 pass 只多 CPU 开销、不写像素）。
            for stage in [RenderPass::Legacy2DOverlay, RenderPass::ScreenUI] {
                if !plan.contains(stage) {
                    continue;
                }
                for layer in stage.overlay_layers() {
                    for index in layer_indices(&mesh.draws, |draw| draw.render_layer, *layer) {
                        let draw = &mesh.draws[index];
                        if draw.sdf {
                            // 文本绘制：共用 pipeline_sdf（shader 按顶点 sdf 值分支）。
                            // bitmap（sdf=2）：Nearest 采样 + MSDF 阈值化，小字号清晰。
                            // MSDF（sdf=1）：Linear 采样 + smoothstep AA，大字号平滑。
                            let bind = if draw.bitmap {
                                self.bind_group_for(draw.texture, &mesh.paths, &mesh.path_handles)
                            } else {
                                self.sdf_bind_for(draw.texture, &mesh.paths)
                            };
                            let Some(bind) = bind else {
                                continue;
                            };
                            pass.set_pipeline(&self.pipeline_sdf);
                            pass.set_bind_group(0, &bind, &[]);
                            pass.draw(draw.start..draw.start + draw.count, 0..1);
                            record_vertex_draw(&mut metrics, draw.count);
                            continue;
                        }
                        let Some(bind) =
                            self.bind_group_for(draw.texture, &mesh.paths, &mesh.path_handles)
                        else {
                            continue;
                        };
                        pass.set_pipeline(&self.pipeline);
                        pass.set_bind_group(0, &bind, &[]);
                        pass.draw(draw.start..draw.start + draw.count, 0..1);
                        record_vertex_draw(&mut metrics, draw.count);
                    }
                }
                executed.push(stage);
            }
        }
        if plan.contains(RenderPass::VectorText)
            && !mesh.vector_vertices.is_empty()
            && !mesh.vector_glyphs.is_empty()
            && !mesh.vector_segments.is_empty()
        {
            let vertex_bytes = bytemuck::cast_slice(&mesh.vector_vertices);
            let glyph_bytes = bytemuck::cast_slice(&mesh.vector_glyphs);
            let segment_bytes = bytemuck::cast_slice(&mesh.vector_segments);
            self.ensure_vector_buffer_capacity(
                vertex_bytes.len() as u64,
                glyph_bytes.len() as u64,
                segment_bytes.len() as u64,
            );
            self.queue
                .write_buffer(&self.vector_vertex_buffer, 0, vertex_bytes);
            let glyph_hash = hash_bytes(glyph_bytes);
            if self.vector_glyph_hash != glyph_hash
                || self.vector_glyph_cache.len() != glyph_bytes.len()
            {
                self.queue
                    .write_buffer(&self.vector_glyph_buffer, 0, glyph_bytes);
                self.vector_glyph_cache.clear();
                self.vector_glyph_cache.extend_from_slice(glyph_bytes);
                self.vector_glyph_hash = glyph_hash;
            }
            let segment_hash = hash_bytes(segment_bytes);
            if self.vector_segment_hash != segment_hash
                || self.vector_segment_cache.len() != segment_bytes.len()
            {
                self.queue
                    .write_buffer(&self.vector_segment_buffer, 0, segment_bytes);
                self.vector_segment_cache.clear();
                self.vector_segment_cache.extend_from_slice(segment_bytes);
                self.vector_segment_hash = segment_hash;
            }
            if self.vector_bind_group.is_none() {
                self.vector_bind_group =
                    Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("rustlua-vector-text-bind-group"),
                        layout: &self.vector_bind_group_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: self.vector_glyph_buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: self.vector_segment_buffer.as_entire_binding(),
                            },
                        ],
                    }));
            }
            let bind = self.vector_bind_group.as_ref().expect("vector bind group");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-vector-text-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: draw_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.vector_pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, self.vector_vertex_buffer.slice(..));
            apply_declared_viewport(&mut pass, overlay_viewport, target);
            pass.draw(0..mesh.vector_vertices.len() as u32, 0..1);
            record_triangle_draw(&mut metrics, mesh.vector_vertices.len() as u32);
            executed.push(RenderPass::VectorText);
        }
        // docs/54 §4.3：最后一条通道——把离屏场景颜色按曝光/色调/暗角写进**目标附件**。
        if plan.contains(RenderPass::PostProcess)
            && let (Some(source), Some(settings)) = (scene_view.as_ref(), post_process)
        {
            let vertices = self.draw_post_process(encoder, source, view, settings, target);
            record_triangle_draw(&mut metrics, vertices as u32);
            executed.push(RenderPass::PostProcess);
        }
        // 把离屏视图放回（下一帧复用；纹理本体一直在 `self` 里，视图只是个句柄）。
        self.post_process_scene_view = scene_view;
        // 计划与执行核对（docs/54 §4.3）：计划驱动的开关如果被运行时条件绕开，就在这里留下
        // 明确的告警与可断言的差异。`last_frame_passes` 与 `last_frame_plan` 都是公开的，
        // 因此"这一帧到底跑了哪些通道"可以在测试里直接断言，不用去数 draw call 反推。
        if let Some(mismatch) = plan.mismatch(&executed) {
            tracing::warn!(
                target: "render",
                "[runtime] render plan mismatch: {mismatch}"
            );
        }
        self.last_frame_plan = plan;
        self.last_frame_passes = executed;
        metrics
    }

    /// 确保 2D 顶点缓冲能容纳 `required` 字节；不足则按 2 的幂扩容。
    ///
    /// 返回扩容后的容量。2D 顶点缓冲不参与任何 bind group（只 `set_vertex_buffer`），
    /// 因此中途重建是安全的。
    fn ensure_2d_vertex_capacity(&mut self, required: u64) -> u64 {
        self.vertex_bytes_peak = self.vertex_bytes_peak.max(required);
        if required <= self.vertex_buffer.size() {
            return self.vertex_buffer.size();
        }
        let capacity = next_2d_vertex_capacity(required);
        tracing::warn!(
            target: "render",
            required,
            previous = self.vertex_buffer.size(),
            capacity,
            growths = self.vertex_buffer_growths + 1,
            "[runtime] 2D vertex buffer grew: {} -> {} bytes (required {})",
            self.vertex_buffer.size(),
            capacity,
            required
        );
        self.vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-vertex-buffer"),
            size: capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.vertex_buffer_growths += 1;
        capacity
    }

    /// 最近一次成功提交的帧统计。GPU timestamp 不可用或尚未完成时为 `None`。
    pub fn last_frame_metrics(&self) -> RenderFrameMetrics {
        self.last_frame_metrics
    }

    /// 上一帧的渲染通道计划（docs/54 §4.3）。
    #[must_use]
    pub fn last_frame_plan(&self) -> &RenderPlan {
        &self.last_frame_plan
    }

    /// 上一帧**实际进入**的通道，按进入顺序。
    ///
    /// 与 [`Self::last_frame_plan`] 不等即意味着某个通道被跳过或顺序变了——渲染器每帧核对一次
    /// 并告警。注意"进入"不等于"产出了 draw call"（`bind_group_for` 找不到纹理时仍可能空转），
    /// 产出量由 [`Self::last_frame_metrics`] 单独计数。
    #[must_use]
    pub fn last_frame_passes(&self) -> &[RenderPass] {
        &self.last_frame_passes
    }

    /// docs/54 §4.4：资源身份缓存的 `(命中, 未命中)` 次数。
    ///
    /// 命中 = 本帧某次贴图解析走的是**句柄**（O(1)），未命中 = 句柄还没进过缓存（首次加载或
    /// 宿主没给身份）。因此这个数字同时是"身份这条路真的被走到"的证据与诊断。
    #[must_use]
    pub fn texture_identity_stats(&self) -> (u64, u64) {
        (self.texture_identity_hits, self.texture_identity_misses)
    }

    /// 已加载的贴图条数（诊断：同一资源不该被加载两次）。
    #[must_use]
    pub fn loaded_texture_count(&self) -> usize {
        self.textures.len()
    }

    /// docs/54 §4.4：网格身份缓存的 `(命中, 未命中)` 次数。
    #[must_use]
    pub fn mesh_identity_stats(&self) -> (u64, u64) {
        (self.mesh_identity_hits, self.mesh_identity_misses)
    }

    /// 已缓存的网格条数（诊断：同一资源不该被解析两次）。
    #[must_use]
    pub fn cached_mesh_count(&self) -> usize {
        self.mesh_cache.len()
    }

    /// 2D 顶点缓冲扩容次数（诊断/验收：稳定场景不应持续增长）。
    pub fn vertex_buffer_growths(&self) -> u64 {
        self.vertex_buffer_growths
    }

    /// 当前 2D 顶点缓冲容量（字节）。
    pub fn vertex_buffer_capacity(&self) -> u64 {
        self.vertex_buffer.size()
    }

    /// 本进程见过的单帧 2D 顶点字节数峰值。
    pub fn vertex_bytes_peak(&self) -> u64 {
        self.vertex_bytes_peak
    }
}

/// Conservative local-space bounding sphere from a primitive's vertex AABB.
fn compute_mesh_bounds(vertices: &[crate::core::mesh3d::MeshVertex]) -> (Vec3, f32) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for vertex in vertices {
        for index in 0..3 {
            min[index] = min[index].min(vertex.position[index]);
            max[index] = max[index].max(vertex.position[index]);
        }
    }
    if !min[0].is_finite() {
        return (Vec3::ZERO, 0.0);
    }
    let center = Vec3::new(
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    );
    let half = Vec3::new(
        (max[0] - min[0]) * 0.5,
        (max[1] - min[1]) * 0.5,
        (max[2] - min[2]) * 0.5,
    );
    let radius = half.length().max(f32::EPSILON);
    (center, radius)
}

/// Builds the joint-palette uniform data padded to `MAX_BONES` identity
/// matrices: `[f32; 4]` rows so `bytemuck` casts the whole buffer at once.
fn joint_palette_data(palette: &[Mat4]) -> Vec<[f32; 4]> {
    let mut data = Vec::with_capacity(crate::core::mesh3d::MAX_BONES * 4);
    for matrix in palette.iter().take(crate::core::mesh3d::MAX_BONES) {
        data.extend_from_slice(&matrix.columns);
    }
    while data.len() < crate::core::mesh3d::MAX_BONES * 4 {
        data.extend_from_slice(&IDENTITY_MAT4);
    }
    data
}

/// 世界 Sprite 顶点缓冲初始容量（按需扩容）。4096 个顶点 ≈ 每帧 1000 多个精灵；
/// 超限时扩容逻辑与 2D overlay 缓冲同思路（2 的幂次增长）。
const WORLD_SPRITE_BUFFER_BYTES: u64 = 4096 * std::mem::size_of::<WorldSpriteVertex>() as u64;

/// P0-①：帧级环形缓冲初始容量（128 KiB ≈ 数百个材质块 + 大量实例矩阵），不足时翻倍。
const FRAME_ARENA_INITIAL_BYTES: u64 = 128 * 1024;

const IDENTITY_MAT4: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn identity_palette_data() -> Vec<[f32; 4]> {
    joint_palette_data(&[])
}

/// Light-space view-projection used by the shadow map: an orthographic-style projection from a camera
/// placed **on the light's side**, looking at the origin.
///
/// `Light::direction` is the direction the light **travels** (`crate::core::light` 与 docs/24 的约定，
/// 着色器用 `-direction` 求表面→光源向量），因此"光源所在的一侧"是 `-direction`：
///
/// ```text
/// light.direction = (0, -1, 0)   // 光自上而下 ⇒ 阴影相机在 +Y（天空一侧）
/// ```
///
/// 这里曾经写成 `+direction`——与着色器**符号相反**：示例场景的 `direction=0.4,0.8,0.5`（按约定
/// 是"光往上照"）因此把阴影相机放到地面之上、而光照却来自下方，结果是所有朝上的表面 `diffuse = 0`
/// （只剩 0.15 环境光）、阴影完全不可见。docs/54 §10 第 ⑩ 行记着这次量测（地面像素恒为环境色、
/// 阴影落点与受光点像素相同）。
fn light_view_projection(light: Light) -> Mat4 {
    let direction = light
        .direction
        .normalized()
        .unwrap_or(Vec3::new(0.0, -1.0, 0.0));
    // 阴影相机**只消费姿态**（`view_matrix`）：它自带一份正交式投影（见下），透视投影的
    // 宽高比在这里从来不参与，因此拆分批 7 之后这台相机也不需要任何视口。
    let Ok(camera) = Camera3D::perspective(-direction * 12.0, Vec3::ZERO, 1.3, 0.1, 30.0) else {
        return Mat4::identity();
    };
    let Ok(view) = camera.view_matrix() else {
        return Mat4::identity();
    };
    let projection = Mat4 {
        columns: [
            [0.1, 0.0, 0.0, 0.0],
            [0.0, 0.1, 0.0, 0.0],
            [0.0, 0.0, -1.0 / 29.9, 0.0],
            [0.0, 0.0, -0.1 / 29.9, 1.0],
        ],
    };
    projection * view
}

pub enum RenderOutcome {
    Presented,
    Retry,
    Skipped,
    Fatal,
}

#[cfg(test)]
mod tests;
