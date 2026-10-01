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
    mesh3d::{GROUND_PLANE_SENTINEL, MeshAsset},
    post_process::PostProcess,
    render_plan::{FrameFacts, RenderPass, RenderPlan},
    text::TextAtlas,
    texture::{TextureAsset, TextureColorSpace},
    window::has_renderable_size,
};
use wgpu::util::DeviceExt;
use wgpu::{
    CompositeAlphaMode, Device, DeviceDescriptor, Instance, InstanceDescriptor, PresentMode, Queue,
    Surface, SurfaceConfiguration, SurfaceError, TextureUsages,
};
use winit::{dpi::PhysicalSize, window::Window};

use crate::host::{
    frame::{
        ATLAS_TEXTURE_PATH, FONT_ATLAS_TEXTURE_PATH, FontAtlasPixels, OverlayVertex, Scene3d,
        SceneMesh, VectorTextVertex, WHITE_TEXTURE_PATH, WorldSpriteVertex,
        font_asset_from_texture_path, is_font_texture_path,
    },
    shader::{
        MESH3D_SHADER, POST_PROCESS_SHADER, SHADOW_SHADER, TEXTURED_SHADER, VECTOR_TEXT_SHADER,
        WORLD_SPRITE_SHADER,
    },
};

/// 需要深度附件的那几条管线（WP6）。把"要不要写深度"从散落的字段字面量收成一处**类型化**
/// 契约，是因为它曾经被脚本化编辑改错位：三个管线里有两条被误改成不写深度，而 golden 只断言
/// 可绘制对象与像素非空、**断言不了深度顺序**，于是 3D 深度静默失效。现在有单测把每条管线的
/// 深度策略逐条钉住（见 `tests::depth_policies_are_pinned_per_pass`）。
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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReadbackMetrics {
    pub slot_count: usize,
    pub busy_count: usize,
    pub submitted: u64,
    pub completed: u64,
    pub dropped: u64,
    pub pending: u64,
    pub cpu_bytes: usize,
    pub buffer_rebuilds: u64,
}

/// 非阻塞离屏回读的结果。
pub enum ReadbackStatus {
    /// 就绪的紧凑 RGBA8 像素（每行 `row_bytes`，无行尾填充），附实际尺寸。
    /// 尺寸可能与当前渲染尺寸不同（排空的是上一帧不同分辨率的槽）。
    Ready {
        pixels: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// 尚未就绪（主线程未阻塞等待 GPU）；调用方沿用上一帧纹理即可。
    Pending,
    /// 回读失败。
    Failed(String),
}

/// 单个回读槽轮询的结果（M11-03）：未就绪 / 就绪像素 / 映射失败。
enum ReadbackPoll {
    NotReady,
    Ready(Vec<u8>, u32, u32),
    Failed(String),
}

/// One asynchronous whole-frame GPU timestamp readback.
struct TimestampSlot {
    resolve_buffer: wgpu::Buffer,
    buffer: wgpu::Buffer,
    submission: Option<wgpu::SubmissionIndex>,
    map_result: std::sync::Arc<std::sync::atomic::AtomicU8>,
    map_error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GpuTimestampState {
    Unsupported,
    Ready,
    MapError,
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
    ) -> Result<Self, String> {
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
    ) -> Result<Self, String> {
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
    ) -> Result<Self, String> {
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
                .map_err(|error| format!("could not create surface: {error}"))?;
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                })
                .await
                .map_err(|error| format!("could not find a compatible GPU adapter: {error}"))?;
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
                .map_err(|error| format!("could not create GPU device: {error}"))?;
            let capabilities = surface.get_capabilities(&adapter);
            let format = capabilities
                .formats
                .iter()
                .copied()
                .find(|format| format.is_srgb())
                .or_else(|| capabilities.formats.first().copied())
                .ok_or_else(|| "GPU surface reported no supported formats".to_owned())?;
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
                .ok_or_else(|| "offscreen renderer requires a device/queue".to_owned())?;
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
        let initial_viewport = Viewport::new(config.width as f32, config.height as f32)
            .map_err(|error| error.to_string())?;
        let camera = Camera3D::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            0.1,
            100.0,
        )
        .map_err(|error| error.to_string())?;
        let camera_matrix = camera
            .view_projection_matrix(initial_viewport)
            .map_err(|error| error.to_string())?;
        let light_camera =
            Camera3D::perspective(Vec3::new(4.0, 6.0, 4.0), Vec3::ZERO, 1.3, 0.1, 30.0)
                .map_err(|error| error.to_string())?;
        let light_view = light_camera
            .view_matrix()
            .map_err(|error| error.to_string())?;
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
            // 32 字节：exposure + vignette + 两个填充位，随后是一个 vec4 着色（见 core::post_process）。
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

    fn create_depth_view(device: &Device, width: u32, height: u32) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("rustlua-depth32-texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    fn create_shadow_view(device: &Device, size: u32) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("rustlua-shadow-map"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Returns the cached geometry + materials for a mesh path, loading it from
    /// the project on first use. Missing or unloadable meshes are skipped with
    /// a warning so a bad reference never crashes the renderer.
    /// docs/54 §4.4：按**资源身份**取网格缓存。
    ///
    /// 有句柄时先查身份表（O(1)），未命中才按路径加载并记下映射；路径键的 `mesh_cache` 保持不变，
    /// 因此"同一网格被两个路径引用"也不会加载两遍。没有句柄的调用（宿主手写 `Scene3d`）仍按路径。
    fn cached_mesh(&mut self, handle: Option<AssetHandle>, path: &str) -> Option<&CachedMesh> {
        if let Some(handle) = handle {
            if let Some(mapped) = self.mesh_slots.get(&handle)
                && self.mesh_cache.contains_key(mapped.as_str())
            {
                self.mesh_identity_hits += 1;
                let key = mapped.clone();
                return self.mesh_cache.get(&key);
            }
            self.mesh_identity_misses += 1;
            if !self.mesh_cache.contains_key(path) {
                self.request_mesh_load(path);
            }
            if self.mesh_cache.contains_key(path) {
                self.mesh_slots.insert(handle, path.to_owned());
            }
            return self.mesh_cache.get(path);
        }
        // 没有身份：旧路径（行为与收敛前一致）。
        if !self.mesh_cache.contains_key(path) {
            self.request_mesh_load(path);
        }
        self.mesh_cache.get(path)
    }

    /// 把已解析的 `MeshAsset` 转成 GPU 侧 `CachedMesh`（上传几何 + albedo）。
    /// 同步与异步路径共用这一份转换，保证两条路径的 GPU 产物完全一致。
    fn cached_from_meshes(&mut self, path: &str, meshes: &[MeshAsset]) -> Option<CachedMesh> {
        let mut primitives = Vec::new();
        for mesh in meshes {
            for primitive in &mesh.primitives {
                let material = mesh.materials.get(primitive.material.unwrap_or(usize::MAX));
                let albedo = material
                    .and_then(|value| value.base_color_image.as_ref())
                    .map(|image| self.upload_albedo(&image.rgba, image.width, image.height));
                primitives.push(CachedPrimitive {
                    geometry: self.upload_geometry(&primitive.vertices, &primitive.indices),
                    // docs/54 §4.3：glTF 的 PBR 参数装进**统一的** `Material`（精灵/Quad 用的是同一个
                    // 类型的无光照变体），因此"缺省材质"只有一处定义。
                    material: crate::core::renderable::Material::pbr(
                        material
                            .map(|value| value.base_color_factor)
                            .unwrap_or([0.55, 0.7, 1.0, 1.0]),
                        // glTF 的底图内嵌在资产里（没有独立路径）：材质里记 `None`，真实贴图身份由
                        // 网格身份 + `albedo` 下标表达（批 I 的结论）。
                        None,
                        material.map(|value| value.metallic_factor).unwrap_or(0.0),
                        material.map(|value| value.roughness_factor).unwrap_or(0.6),
                    ),
                    albedo,
                    bounds: compute_mesh_bounds(&primitive.vertices),
                });
            }
        }
        if primitives.is_empty() {
            tracing::warn!(target: "render", path = %path, "[render] 3D mesh has no primitives; skipped");
            return None;
        }
        Some(CachedMesh { primitives })
    }

    /// P0-②：网格 MISS 的异步请求 + 收完成品上传。Async 下资源后台解码、就绪后入缓存；
    /// Sync 下 request 同步完成、同一调用内即入缓存。
    fn request_mesh_load(&mut self, path: &str) {
        if path == GROUND_PLANE_SENTINEL {
            // 地面哨兵不走文件加载（也没有异步文件）；直接同步合成并缓存。
            self.mesh_cache
                .insert(path.to_owned(), self.ground_plane_cached_mesh());
            return;
        }
        let resolved = self
            .project_root
            .as_ref()
            .map(|root| root.join(path))
            .unwrap_or_else(|| PathBuf::from(path));
        self.async_loader.request_gltf(path, &resolved);
        let drained = self.async_loader.drain_ready();
        self.upload_decoded(drained);
    }

    /// P0-②：主线程把本轮完成的异步解码产物做 GPU 上传并写入缓存。
    ///
    /// 这是"后台解码、主线程小步上传"的上传侧：`wgpu` 的提交本身不阻塞，真正卡主线程的
    /// CPU 解码已全部下移到 worker。失败资源只记 warning（调用方已有占位回退），不崩溃。
    fn upload_decoded(&mut self, assets: Vec<crate::core::async_loader::DecodedAsset>) {
        for asset in assets {
            if let Some(error) = &asset.error {
                tracing::warn!(
                    target: "render",
                    path = %asset.key,
                    error = %error,
                    "[render] async asset load failed; fallback used"
                );
                continue;
            }
            match asset.kind {
                crate::core::async_loader::LoadKind::TexturePng => {
                    if let Some(texture) = asset.texture {
                        if self.texture_paths.contains_key(asset.key.as_str()) {
                            continue; // 已上传过（并发请求去重后的兜底）。
                        }
                        match self.upload_texture(&texture, &asset.key) {
                            Some(bind) => {
                                self.texture_paths
                                    .insert(asset.key.clone(), self.textures.len());
                                self.textures.push(LoadedTexture { bind_group: bind });
                                tracing::info!(
                                    target: "render",
                                    path = %asset.key,
                                    dimensions = ?(texture.width(), texture.height()),
                                    "[render] texture loaded (async)"
                                );
                            }
                            None => {
                                tracing::warn!(
                                    target: "render",
                                    path = %asset.key,
                                    "[render] texture GPU upload failed; fallback used"
                                );
                            }
                        }
                    }
                }
                crate::core::async_loader::LoadKind::GltfMesh => {
                    if let Some(meshes) = asset.meshes {
                        if self.mesh_cache.contains_key(&asset.key) {
                            continue;
                        }
                        if let Some(cached) = self.cached_from_meshes(&asset.key, &meshes) {
                            self.mesh_cache.insert(asset.key.clone(), cached);
                            tracing::info!(
                                target: "render",
                                path = %asset.key,
                                "[render] 3D mesh cached (async)"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Uploads decoded base-color pixels (RGBA8) as an sRGB GPU texture and
    /// returns its index in `albedo_textures`.
    fn upload_albedo(&mut self, rgba: &[u8], width: u32, height: u32) -> u32 {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-albedo-texture"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width.max(1)),
                rows_per_image: Some(height.max(1)),
            },
            wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.albedo_textures
            .push(CachedAlbedoTexture { texture, view });
        tracing::info!(
            target: "render",
            index = self.albedo_textures.len() - 1,
            width,
            height,
            "[render] 3D albedo texture uploaded"
        );
        (self.albedo_textures.len() - 1) as u32
    }

    /// Synthesizes the built-in ground receiver plane for the
    /// `__ground_plane__` MeshRenderer sentinel so shadows have a surface.
    fn ground_plane_cached_mesh(&self) -> CachedMesh {
        let vertices = [
            crate::core::mesh3d::MeshVertex {
                position: [-4.0, 0.0, -4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [0.0, 0.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            crate::core::mesh3d::MeshVertex {
                position: [4.0, 0.0, -4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [1.0, 0.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            crate::core::mesh3d::MeshVertex {
                position: [4.0, 0.0, 4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [1.0, 1.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            crate::core::mesh3d::MeshVertex {
                position: [-4.0, 0.0, 4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [0.0, 1.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
        ];
        CachedMesh {
            primitives: vec![CachedPrimitive {
                geometry: self.upload_geometry(&vertices, &[0, 2, 1, 0, 3, 2]),
                material: crate::core::renderable::Material::pbr(
                    [0.25, 0.3, 0.38, 1.0],
                    None,
                    0.0,
                    0.9,
                ),
                albedo: None,
                bounds: compute_mesh_bounds(&vertices),
            }],
        }
    }

    fn upload_geometry(
        &self,
        vertices: &[crate::core::mesh3d::MeshVertex],
        indices: &[u32],
    ) -> GpuGeometry {
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-mesh3d-vertices"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-mesh3d-indices"),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        GpuGeometry {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
        }
    }

    /// P0-①：把一块 uniform 数据写入材质/调色板帧级环形缓冲，返回
    /// （所属缓冲句柄, 起始偏移, 字节数）。句柄单独返回是为了覆盖"本帧中途扩容"
    /// 的情形：扩容前已写出的块仍在旧缓冲里，旧句柄克隆保证它们存活且指向正确。
    fn push_frame_uniform(&mut self, contents: &[u8]) -> (wgpu::Buffer, u64, u64) {
        let mask = self.uniform_align - 1;
        let size = contents.len() as u64;
        let mut offset = self.material_arena_used.wrapping_add(mask) & !mask;
        if offset + size > self.material_arena.size() {
            let grown = (self.material_arena.size() * 2).max(offset + size);
            self.material_arena = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-material3d-frame-arena"),
                size: grown,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            offset = 0;
        }
        self.queue
            .write_buffer(&self.material_arena, offset, contents);
        self.material_arena_used = offset + size;
        (self.material_arena.clone(), offset, size)
    }

    /// P0-①：实例模型矩阵帧级环形缓冲的写入端，语义与 [`Self::push_frame_uniform`] 相同。
    fn push_frame_storage(&mut self, contents: &[u8]) -> (wgpu::Buffer, u64, u64) {
        let mask = self.storage_align - 1;
        let size = contents.len() as u64;
        let mut offset = self.instance_arena_used.wrapping_add(mask) & !mask;
        if offset + size > self.instance_arena.size() {
            let grown = (self.instance_arena.size() * 2).max(offset + size);
            self.instance_arena = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-instance-models-frame-arena"),
                size: grown,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            offset = 0;
        }
        self.queue
            .write_buffer(&self.instance_arena, offset, contents);
        self.instance_arena_used = offset + size;
        (self.instance_arena.clone(), offset, size)
    }

    /// Creates a material bind group carrying one primitive's file material,
    /// the joint palette (identity for static meshes, animated for skinned
    /// drawables), and the per-instance model matrices. P0-①：数据落在帧级环形
    /// 缓冲里按偏移引用，每次调用不再新建任何 GPU buffer。
    fn material_bind_group(
        &mut self,
        material: GpuMaterial,
        palette: &[Mat4],
        models: &[Mat4],
    ) -> wgpu::BindGroup {
        let material_data: [[f32; 4]; 2] = [
            material.base_color,
            [
                material.metallic,
                material.roughness,
                if material.skinned { 1.0 } else { 0.0 },
                0.0,
            ],
        ];
        let (material_buffer, material_offset, material_size) =
            self.push_frame_uniform(bytemuck::cast_slice(&material_data));
        let (palette_buffer, palette_offset, palette_size) =
            if material.skinned && !palette.is_empty() {
                self.push_frame_uniform(bytemuck::cast_slice(&joint_palette_data(palette)))
            } else {
                let size = self.identity_palette.size();
                (self.identity_palette.clone(), 0, size)
            };
        let mut instance_data = Vec::with_capacity(models.len() * 4);
        for model in models {
            instance_data.extend_from_slice(&model.columns);
        }
        let (instance_buffer, instance_offset, instance_size) =
            self.push_frame_storage(bytemuck::cast_slice(&instance_data));
        let albedo = material
            .albedo
            .and_then(|index| self.albedo_textures.get(index as usize))
            .map_or(&self.white_albedo_view, |cached| &cached.view);
        fn buffer_binding<'a>(
            buffer: &'a wgpu::Buffer,
            offset: u64,
            size: u64,
        ) -> wgpu::BindingResource<'a> {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset,
                size: std::num::NonZeroU64::new(size),
            })
        }
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-material3d-frame-bind-group"),
            layout: &self._material_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer_binding(&material_buffer, material_offset, material_size),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buffer_binding(&palette_buffer, palette_offset, palette_size),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buffer_binding(&instance_buffer, instance_offset, instance_size),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(albedo),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.albedo_sampler),
                },
            ],
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

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if !has_renderable_size(size.width, size.height) {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        self.ensure_depth_view(size.width, size.height);
        tracing::info!(target: "game",
            "[render] surface reconfigured: {}x{}",
            size.width, size.height
        );
    }

    /// Rebuilds the depth view when the render target size changes (window
    /// resize or an offscreen target of a different size).
    fn ensure_depth_view(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.depth_size != (width, height) {
            self.depth_view = Self::create_depth_view(&self.device, width, height);
            self.depth_size = (width, height);
        }
    }

    /// docs/54 §4.3：确保后处理的场景离屏颜色纹理与目标同尺寸。
    ///
    /// 尺寸变化才重建（与深度附件同一策略）。视图单独存进 `self.post_process_scene_view`，
    /// 渲染时被"取出 → 用完放回"，因此渲染循环里不需要同时借 `&self` 与 `&mut self`。
    fn ensure_post_process_scene_view(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.post_process_size == (width, height) && self.post_process_scene_view.is_some() {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-post-process-scene"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // 与目标附件同格式：采样时 sRGB 自动线性化、写回时再编码，恒等参数因此近似不变。
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.post_process_scene_view =
            Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.post_process_texture = Some(texture);
        self.post_process_size = (width, height);
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

    /// Copies an RGBA8 texture back to the CPU (synchronous readback), used by
    /// the editor to display the embedded preview. The texture must have
    /// `COPY_SRC` usage.
    pub fn copy_texture_to_cpu(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        // `bytes_per_row` must be a multiple of 256 for `copy_texture_to_buffer`.
        let row_bytes = 4 * width;
        let bytes_per_row = row_bytes.div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-preview-readback"),
            size: (bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-preview-copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let map_result = rx
            .recv()
            .map_err(|error| format!("preview readback channel error: {error}"))?;
        map_result.map_err(|error| format!("preview texture map failed: {error}"))?;
        // 去掉行尾填充，返回紧凑 RGBA8（每行 `row_bytes`）。
        let padded = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((row_bytes * height) as usize);
        for row in 0..height as usize {
            let start = row * bytes_per_row as usize;
            pixels.extend_from_slice(&padded[start..start + row_bytes as usize]);
        }
        Ok(pixels)
    }

    /// 非阻塞离屏回读：总是提交一次纹理拷贝（写进环中下一空闲槽），同时尝试
    /// 读取之前提交的结果。就绪返回 `Ready(pixels)`，未就绪返回 `Pending`——
    /// 主线程永不阻塞等待 GPU。纹理须有 `COPY_SRC` usage。
    ///
    /// M11-03（R-04）：映射失败的回读槽现在会返回 `Failed` 而非当作就绪返回脏数据；
    /// 失败槽会清空并允许下帧重试。
    pub fn readback_frame(
        &mut self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> ReadbackStatus {
        // 先排空任意就绪槽（滞后一帧的结果）；失败槽上报错误（也允许重试）。
        let mut ready = None;
        let mut failed = None;
        for index in 0..self.readback_slots.len() {
            match self.poll_readback(index) {
                ReadbackPoll::Ready(pixels, width, height) => {
                    ready = Some((pixels, width, height));
                    break;
                }
                ReadbackPoll::Failed(error) => {
                    if failed.is_none() {
                        failed = Some(error);
                    }
                }
                ReadbackPoll::NotReady => {}
            }
        }
        // 无论是否排空到结果，都提交本次拷贝，保证每个渲染帧都会被回读。
        self.submit_readback(texture, width, height);
        self.readback_pending_count += 1;
        if let Some((pixels, width, height)) = ready {
            ReadbackStatus::Ready {
                pixels,
                width,
                height,
            }
        } else if let Some(error) = failed {
            ReadbackStatus::Failed(error)
        } else {
            ReadbackStatus::Pending
        }
    }

    pub fn readback_slot_count(&self) -> usize {
        self.readback_slots.len()
    }

    pub fn readback_busy_count(&self) -> usize {
        self.readback_slots
            .iter()
            .filter(|slot| slot.submission.is_some())
            .count()
    }

    pub fn readback_metrics(&self) -> ReadbackMetrics {
        ReadbackMetrics {
            slot_count: self.readback_slots.len(),
            busy_count: self.readback_busy_count(),
            submitted: self.readback_submitted,
            completed: self.readback_completed,
            dropped: self.readback_dropped,
            pending: self.readback_pending_count,
            cpu_bytes: self
                .readback_slots
                .iter()
                .map(|slot| slot.pixels.capacity())
                .sum(),
            buffer_rebuilds: self.readback_buffer_rebuilds,
        }
    }
    /// 是否有在途（已提交未读）的回读等待排空。
    pub fn readback_in_flight(&self) -> bool {
        self.readback_slots
            .iter()
            .any(|slot| slot.submission.is_some())
    }

    /// 仅排空待读的回读（内容未变、无需新提交时调用），返回就绪像素或映射错误。
    pub fn readback_pending(&mut self) -> ReadbackStatus {
        for index in 0..self.readback_slots.len() {
            match self.poll_readback(index) {
                ReadbackPoll::Ready(pixels, width, height) => {
                    return ReadbackStatus::Ready {
                        pixels,
                        width,
                        height,
                    };
                }
                ReadbackPoll::Failed(error) => return ReadbackStatus::Failed(error),
                ReadbackPoll::NotReady => {}
            }
        }
        ReadbackStatus::Pending
    }

    /// 非阻塞检查一个回读槽的映射结果：未就绪 / 就绪（读出行数据并置回空闲）/
    /// 映射失败（上报错误并把槽置回空闲以便重试）。
    fn poll_readback(&mut self, index: usize) -> ReadbackPoll {
        let Some(slot) = self.readback_slots.get(index) else {
            return ReadbackPoll::NotReady;
        };
        let Some(_) = slot.submission else {
            return ReadbackPoll::NotReady;
        };
        // 非阻塞 maintain：处理所有已完成的提交与 `map_async` 回调（超时即返回，
        // 主线程不阻塞）。就绪与否由 `map_result` 标志判断。
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::ZERO),
        });
        let result = slot.map_result.load(std::sync::atomic::Ordering::Acquire);
        if result == 0 {
            return ReadbackPoll::NotReady;
        }
        if result == 2 {
            // 映射失败：上报错误并把槽置回空闲（buffer 未映射，不能 unmap）。
            let error = slot
                .map_error
                .lock()
                .ok()
                .and_then(|mut guard| guard.take())
                .unwrap_or_else(|| "GPU 回读映射失败".to_owned());
            let slot = &mut self.readback_slots[index];
            slot.submission = None;
            slot.map_result
                .store(0, std::sync::atomic::Ordering::Release);
            return ReadbackPoll::Failed(error);
        }
        // 映射成功：读出行数据并置回空闲。
        let (width, height, row_bytes, bytes_per_row) =
            (slot.width, slot.height, slot.row_bytes, slot.bytes_per_row);
        let buffer = slot.buffer.clone();
        let padded = buffer.slice(..).get_mapped_range();
        let mut pixels = self.readback_slots[index].pixels.clone();
        pixels.clear();
        pixels.reserve((row_bytes * height) as usize);
        for row in 0..height as usize {
            let start = row * bytes_per_row as usize;
            pixels.extend_from_slice(&padded[start..start + row_bytes as usize]);
        }
        drop(padded);
        buffer.unmap();
        // 槽置回空闲，允许复用。
        let slot = &mut self.readback_slots[index];
        slot.submission = None;
        slot.map_result
            .store(0, std::sync::atomic::Ordering::Release);
        self.readback_completed = self.readback_completed.saturating_add(1);
        ReadbackPoll::Ready(pixels, width, height)
    }

    /// 向一个空闲回读槽提交离屏纹理 → staging buffer 的拷贝（buffer 尺寸不符时重建）。
    fn submit_readback(&mut self, texture: &wgpu::Texture, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let row_bytes = 4 * width;
        let bytes_per_row = row_bytes.div_ceil(256);
        let bytes_per_row = bytes_per_row * 256;
        let size_bytes = (bytes_per_row * height) as u64;
        tracing::debug!(target: "render", width, height, "[readback] submit");
        // 惰性创建双缓冲槽。
        while self.readback_slots.len() < 2 {
            self.readback_slots.push(ReadbackSlot {
                buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("rustlua-editor-readback"),
                    size: size_bytes.max(4),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                submission: None,
                map_result: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0)),
                map_error: std::sync::Arc::new(std::sync::Mutex::new(None)),
                width,
                height,
                row_bytes,
                bytes_per_row,
                pixels: Vec::with_capacity((row_bytes * height) as usize),
                generation: 0,
            });
        }
        // 找一个空闲槽（无在途提交）。
        let Some(index) = self
            .readback_slots
            .iter()
            .position(|slot| slot.submission.is_none())
        else {
            self.readback_dropped += 1;
            return; // 全部在途，跳过本次提交
        };
        // 尺寸不符时重建 buffer，避免越界写入。
        let slot = &mut self.readback_slots[index];
        if slot.width != width || slot.height != height || slot.buffer.size() < size_bytes {
            slot.buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-editor-readback"),
                size: size_bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            slot.width = width;
            slot.height = height;
            slot.row_bytes = row_bytes;
            slot.bytes_per_row = bytes_per_row;
            slot.generation = slot.generation.saturating_add(1);
            self.readback_buffer_rebuilds = self.readback_buffer_rebuilds.saturating_add(1);
        }
        let buffer = slot.buffer.clone();
        let map_result = slot.map_result.clone();
        let map_error = slot.map_error.clone();
        slot.map_result
            .store(0, std::sync::atomic::Ordering::Release);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-editor-readback-copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        // 先提交拷贝，再请求映射（与同步回读一致：map_async 会把 buffer 标记为
        // mapped-pending，若在 copy 提交前调用会触发 "Buffer is still mapped"）。
        let submission = self.queue.submit([encoder.finish()]);
        // M11-03（R-04）：捕获 `map_async` 结果。映射失败不再被当作"已就绪"返回脏数据，
        // 而是记录错误（map_result=2 + map_error），由 poll_readback 上报 Failed 并清槽重试。
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| match result {
                Ok(()) => map_result.store(1, std::sync::atomic::Ordering::Release),
                Err(error) => {
                    map_result.store(2, std::sync::atomic::Ordering::Release);
                    if let Ok(mut guard) = map_error.lock() {
                        *guard = Some(error.to_string());
                    }
                }
            });
        slot.submission = Some(submission);
        self.readback_submitted = self.readback_submitted.saturating_add(1);
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

    fn acquire_timestamp_slot(&mut self) -> Option<usize> {
        if self.timestamp_state != GpuTimestampState::Ready {
            return None;
        }
        if let Some(index) = self
            .timestamp_slots
            .iter()
            .position(|slot| slot.submission.is_none())
        {
            return Some(index);
        }
        if self.timestamp_slots.len() >= 3 {
            return None;
        }
        let map_result = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
        let map_error = std::sync::Arc::new(std::sync::Mutex::new(None));
        self.timestamp_slots.push(TimestampSlot {
            resolve_buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-frame-timestamp-resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-frame-timestamp-readback"),
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            submission: None,
            map_result,
            map_error,
        });
        Some(self.timestamp_slots.len() - 1)
    }

    fn finish_gpu_timestamp(&self, encoder: &mut wgpu::CommandEncoder, slot_index: usize) {
        let Some(query_set) = &self.timestamp_query_set else {
            return;
        };
        encoder.write_timestamp(query_set, 1);
        let slot = &self.timestamp_slots[slot_index];
        encoder.resolve_query_set(query_set, 0..2, &slot.resolve_buffer, 0);
        encoder.copy_buffer_to_buffer(&slot.resolve_buffer, 0, &slot.buffer, 0, 16);
    }

    fn arm_timestamp_slot(&mut self, index: usize, submission: wgpu::SubmissionIndex) {
        let slot = &mut self.timestamp_slots[index];
        let result = std::sync::Arc::clone(&slot.map_result);
        let error = std::sync::Arc::clone(&slot.map_error);
        slot.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |mapped| match mapped {
                Ok(()) => result.store(1, std::sync::atomic::Ordering::Release),
                Err(cause) => {
                    result.store(2, std::sync::atomic::Ordering::Release);
                    if let Ok(mut guard) = error.lock() {
                        *guard = Some(cause.to_string());
                    }
                }
            });
        slot.submission = Some(submission);
    }

    fn poll_gpu_timestamp(&mut self) -> Option<u64> {
        if self.timestamp_state != GpuTimestampState::Ready {
            return None;
        }
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::ZERO),
        });
        for slot in &mut self.timestamp_slots {
            if slot.submission.is_none() {
                continue;
            }
            match slot.map_result.load(std::sync::atomic::Ordering::Acquire) {
                0 => continue,
                1 => {
                    let bytes = slot.buffer.slice(..).get_mapped_range();
                    let begin =
                        u64::from_le_bytes(bytes[0..8].try_into().expect("timestamp begin"));
                    let end = u64::from_le_bytes(bytes[8..16].try_into().expect("timestamp end"));
                    drop(bytes);
                    slot.buffer.unmap();
                    slot.submission = None;
                    slot.map_result
                        .store(0, std::sync::atomic::Ordering::Release);
                    let nanos = end.saturating_sub(begin) as f64 * self.timestamp_period_ns as f64;
                    return Some((nanos / 1_000.0).round() as u64);
                }
                _ => {
                    self.timestamp_error = slot
                        .map_error
                        .lock()
                        .ok()
                        .and_then(|mut value| value.take());
                    slot.submission = None;
                    slot.map_result
                        .store(0, std::sync::atomic::Ordering::Release);
                    self.timestamp_state = GpuTimestampState::MapError;
                    return None;
                }
            }
        }
        None
    }

    /// Waits briefly for one timestamp result. Intended for offline benchmark
    /// collection; the normal render path never blocks on GPU timing.
    pub fn wait_for_gpu_timestamp(&mut self, timeout: std::time::Duration) -> Option<u64> {
        if self.timestamp_state != GpuTimestampState::Ready {
            return None;
        }
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(value) = self.poll_gpu_timestamp() {
                return Some(value);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            let _ = self.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_millis(1)),
            });
        }
    }

    /// Whether this device was created with timestamp query support.
    pub fn gpu_timestamp_supported(&self) -> bool {
        self.timestamp_state != GpuTimestampState::Unsupported
    }

    /// Stable diagnostic for benchmark reports when no GPU timestamp is available.
    pub fn gpu_timestamp_reason(&self) -> Option<&str> {
        match self.timestamp_state {
            GpuTimestampState::Unsupported => Some("unsupported"),
            GpuTimestampState::MapError => self.timestamp_error.as_deref().or(Some("map_error")),
            GpuTimestampState::Ready => None,
        }
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

    fn ensure_vector_buffer_capacity(
        &mut self,
        vertex_size: u64,
        glyph_size: u64,
        segment_size: u64,
    ) {
        fn grown(current: u64, required: u64) -> u64 {
            if required <= current {
                current
            } else {
                required.max(256).next_power_of_two()
            }
        }
        let mut buffers_changed = false;
        let vertex_capacity = grown(self.vector_vertex_buffer.size(), vertex_size);
        if vertex_capacity != self.vector_vertex_buffer.size() {
            self.vector_vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-vector-text-vertex-buffer"),
                size: vertex_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            buffers_changed = true;
        }
        let glyph_capacity = grown(self.vector_glyph_buffer.size(), glyph_size);
        if glyph_capacity != self.vector_glyph_buffer.size() {
            self.vector_glyph_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-vector-text-glyph-buffer"),
                size: glyph_capacity,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            buffers_changed = true;
        }
        let segment_capacity = grown(self.vector_segment_buffer.size(), segment_size);
        if segment_capacity != self.vector_segment_buffer.size() {
            self.vector_segment_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-vector-text-segment-buffer"),
                size: segment_capacity,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            buffers_changed = true;
        }
        if buffers_changed {
            self.vector_bind_group = None;
            self.vector_glyph_cache.clear();
            self.vector_segment_cache.clear();
            self.vector_glyph_hash = 0;
            self.vector_segment_hash = 0;
        }
    }

    /// Picks the SDF bind group for a draw: the default-atlas bind for the the
    /// `FONT_ATLAS_TEXTURE_PATH` sentinel, or the per-custom-font bind for a
    /// namespaced `font://...` path. Returns `None` when the target atlas hasn't
    /// been uploaded this frame yet (caller skips the draw).
    fn sdf_bind_for(&self, texture: u32, paths: &[String]) -> Option<wgpu::BindGroup> {
        if texture == 0 {
            return None;
        }
        let index = texture as usize - 1;
        let path = paths.get(index)?;
        if path == FONT_ATLAS_TEXTURE_PATH {
            return self.font_atlas_sdf_bind.clone();
        }
        if is_font_texture_path(path) {
            return self
                .custom_fonts
                .get(path)
                .and_then(|font| font.sdf_bind.clone());
        }
        None
    }

    /// Returns the bind group for a draw: the white texture for `texture == 0`
    /// or the white sentinel path, the built-in glyph atlas for the atlas
    /// sentinel path, the per-custom-font bind for a `font://...` path, otherwise
    /// the cached (or freshly loaded) asset at `paths[texture - 1]`.
    fn bind_group_for(
        &mut self,
        texture: u32,
        paths: &[String],
        handles: &[AssetHandle],
    ) -> Option<wgpu::BindGroup> {
        if texture == 0 {
            return self.ensure_white();
        }
        let path = &paths[texture as usize - 1];
        if path == WHITE_TEXTURE_PATH {
            return self.ensure_white();
        }
        if path == ATLAS_TEXTURE_PATH {
            return self.ensure_atlas();
        }
        if path == FONT_ATLAS_TEXTURE_PATH {
            return self.font_atlas_bind.clone();
        }
        if is_font_texture_path(path) {
            return self
                .custom_fonts
                .get(path)
                .and_then(|font| font.bind.clone());
        }
        // docs/54 §4.4：宿主给了资源身份就按**句柄**查缓存（O(1)，并保证"同路径恒同句柄"）。
        // 宿主手写 `SceneMesh` 时没有身份（`path_handles` 为空/短），此时回退到按路径线性扫描，
        // 行为与收敛前完全一致。
        if let Some(handle) = handles.get(texture as usize - 1).copied() {
            if let Some(slot) = self.texture_slots.get(&handle).copied()
                && let Some(entry) = self.textures.get(slot)
            {
                self.texture_identity_hits += 1;
                return Some(entry.bind_group.clone());
            }
            self.texture_identity_misses += 1;
        }
        // P0-③：按路径查缓存走 HashMap（O(1)）。`textures` 只追加，索引恒有效。
        if let Some(&slot) = self.texture_paths.get(path.as_str()) {
            if let Some(handle) = handles.get(texture as usize - 1).copied() {
                self.texture_slots.insert(handle, slot);
            }
            return Some(self.textures[slot].bind_group.clone());
        }
        let resolved = self
            .project_root
            .as_ref()
            .map(|root| root.join(path))
            .unwrap_or_else(|| PathBuf::from(path));
        // P0-②：MISS → 异步请求（幂等）并收走本轮完成的解码产物上传。
        // Async 下若本路径未就绪，先回退白贴图，就绪后后续帧自动命中真实贴图；
        // Sync 下请求同步完成、同一调用内即命中，golden 逐字节不变。
        self.async_loader.request_texture(path, &resolved);
        let drained = self.async_loader.drain_ready();
        if !drained.is_empty() {
            self.upload_decoded(drained);
        }
        if let Some(&slot) = self.texture_paths.get(path.as_str()) {
            if let Some(handle) = handles.get(texture as usize - 1).copied() {
                self.texture_slots.insert(handle, slot);
            }
            return Some(self.textures[slot].bind_group.clone());
        }
        tracing::debug!(target: "render", path = %path, "[render] texture not ready yet; white fallback this frame");
        self.ensure_white()
    }

    fn ensure_white(&mut self) -> Option<wgpu::BindGroup> {
        if let Some(white) = &self.white {
            return Some(white.clone());
        }
        let asset =
            TextureAsset::from_rgba8(1, 1, vec![255, 255, 255, 255], TextureColorSpace::Srgb)
                .ok()?;
        let bind = self.upload_texture(&asset, "rustlua-white-1x1")?;
        self.white = Some(bind.clone());
        Some(bind)
    }

    fn ensure_atlas(&mut self) -> Option<wgpu::BindGroup> {
        if let Some(atlas) = &self.atlas {
            return Some(atlas.clone());
        }
        let built_in = TextAtlas::built_in();
        let asset = TextureAsset::from_rgba8(
            built_in.width(),
            built_in.height(),
            built_in.pixels().to_vec(),
            TextureColorSpace::Srgb,
        )
        .ok()?;
        let bind = self.upload_texture(&asset, "rustlua-builtin-text-atlas")?;
        self.atlas = Some(bind.clone());
        Some(bind)
    }

    /// Uploads the CJK font atlas texture when its version changed.
    fn ensure_font_atlas(&mut self, font: &FontAtlasPixels) -> Option<wgpu::BindGroup> {
        if font.pixels.is_empty() {
            // 空像素 = 上游声明"自上次上传未变化"。此时若本渲染器从未上传过该
            // 版本（新渲染器 / 上次版本被空网格帧消费），不能续用旧 bind（内容
            // 不匹配），也不得以空数据 write_texture 整张图集（wgpu 校验崩溃）；
            // 返回 None 让调用方跳过本帧 SDF 绘制，等下一帧全量像素再上传。
            return if self.font_atlas_uploaded_version == font.version {
                self.font_atlas_bind.clone()
            } else {
                None
            };
        }
        let pixels_hash = hash_bytes(&font.pixels);
        if font.version == self.font_atlas_uploaded_version
            && pixels_hash == self.font_atlas_uploaded_hash
        {
            return self.font_atlas_bind.clone();
        }
        // 上传前仍复核像素量与纹理面积一致（纵深防御；上游协议破坏时降级为
        // 跳过本帧，而不是 wgpu 校验 panic 闪退）。
        if (font.pixels.len() as u64) != (font.width as u64 * font.height as u64 * 4) {
            tracing::warn!(
                target: "render",
                expected = font.width as u64 * font.height as u64 * 4,
                actual = font.pixels.len() as u64,
                "[render] font atlas pixel size mismatch; skipping upload this frame"
            );
            return None;
        }
        // SDF 距离场不能走 sRGB 解码（会非线性化距离斜坡），用 Rgba8Unorm。
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let texture = if self.font_atlas_texture.as_ref().is_some_and(|texture| {
            let size = texture.size();
            size.width == font.width && size.height == font.height
        }) {
            self.font_atlas_texture.as_ref().unwrap().clone()
        } else {
            self.font_atlas_bind = None;
            self.font_atlas_sdf_bind = None;
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("rustlua-font-atlas"),
                size: wgpu::Extent3d {
                    width: font.width,
                    height: font.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.font_atlas_texture = Some(texture.clone());
            texture
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &font.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(font.width * 4),
                rows_per_image: Some(font.height),
            },
            wgpu::Extent3d {
                width: font.width,
                height: font.height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-font-atlas-bind"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.font_atlas_uploaded_version = font.version;
        self.font_atlas_uploaded_hash = pixels_hash;
        self.font_atlas_bind = Some(bind_group.clone());
        // SDF 文本用 Linear 采样绑定（同一纹理，不同采样器）。
        let sdf_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-font-atlas-sdf-bind"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sdf_sampler),
                },
            ],
        });
        self.font_atlas_sdf_bind = Some(sdf_bind);
        Some(bind_group)
    }

    /// Uploads/re-binds one per-Text custom font atlas when its version changed.
    /// Mirrors [`Self::ensure_font_atlas`] but keyed by the namespaced `font://...`
    /// id, so each custom font samples its own SDF texture.
    fn ensure_custom_font(&mut self, key: &str, font: &FontAtlasPixels) -> Option<()> {
        if font.pixels.is_empty() {
            // 空像素 = 上游声明"自上次上传未变化"。与默认图集同一防线：只有当本
            // 渲染器**已经上传过该版本**时才可以续用现有 bind。否则（新渲染器、
            // 或上一帧是空网格帧而被整体跳过）必须丢弃过期 bind，让本帧跳过该字
            // 体的 SDF 绘制，等下一次全量像素到达再上传——否则会永久画出上一次
            // 的字形，表现为"改了文案/字号却一直显示旧字"或整段缺字。
            let stale = self
                .custom_fonts
                .get(key)
                .is_none_or(|entry| entry.uploaded_version != font.version);
            if stale && let Some(entry) = self.custom_fonts.get_mut(key) {
                let had_bind = entry.sdf_bind.is_some();
                entry.bind = None;
                entry.sdf_bind = None;
                if had_bind {
                    tracing::debug!(
                        target: "render",
                        key,
                        version = font.version,
                        uploaded = entry.uploaded_version,
                        "[render] custom font atlas version changed without pixels; \
                         dropping stale bind for this frame"
                    );
                }
            }
            return Some(());
        }
        let pixels_hash = hash_bytes(&font.pixels);
        if self.custom_fonts.get(key).is_some_and(|entry| {
            entry.uploaded_version == font.version && entry.uploaded_hash == pixels_hash
        }) {
            return Some(());
        }
        // 上传前复核像素量与纹理面积一致（纵深防御，见 ensure_font_atlas）。
        if (font.pixels.len() as u64) != (font.width as u64 * font.height as u64 * 4) {
            tracing::warn!(
                target: "render",
                key,
                expected = font.width as u64 * font.height as u64 * 4,
                actual = font.pixels.len() as u64,
                "[render] custom font atlas pixel size mismatch; skipping upload this frame"
            );
            return Some(());
        }
        // SDF 距离场不能走 sRGB 解码，用 Rgba8Unorm（与默认图集一致）。
        let format = wgpu::TextureFormat::Rgba8Unorm;
        // 尺寸未变则复用现有纹理（只重写像素）；尺寸变了重建并丢弃旧 bind。
        let reuse = self.custom_fonts.get(key).and_then(|entry| {
            let texture = entry.texture.as_ref()?;
            let size = texture.size();
            (size.width == font.width && size.height == font.height).then(|| texture.clone())
        });
        let texture = match reuse {
            Some(texture) => texture,
            None => self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!(
                    "rustlua-custom-font-{}",
                    font_asset_from_texture_path(key)
                )),
                size: wgpu::Extent3d {
                    width: font.width,
                    height: font.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            }),
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &font.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(font.width * 4),
                rows_per_image: Some(font.height),
            },
            wgpu::Extent3d {
                width: font.width,
                height: font.height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-custom-font-bind"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let sdf_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-custom-font-sdf-bind"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sdf_sampler),
                },
            ],
        });
        self.custom_fonts.insert(
            key.to_owned(),
            CustomFontUpload {
                texture: Some(texture),
                bind: Some(bind),
                sdf_bind: Some(sdf_bind),
                uploaded_version: font.version,
                uploaded_hash: pixels_hash,
            },
        );
        Some(())
    }

    fn upload_texture(&self, asset: &TextureAsset, label: &str) -> Option<wgpu::BindGroup> {
        let format = match asset.color_space() {
            TextureColorSpace::Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            TextureColorSpace::Linear => wgpu::TextureFormat::Rgba8Unorm,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: asset.width(),
                height: asset.height(),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            asset.pixels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(asset.bytes_per_row()),
                rows_per_image: Some(asset.height()),
            },
            wgpu::Extent3d {
                width: asset.width(),
                height: asset.height(),
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Some(bind_group)
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
/// `Light::direction` is the direction the light **travels** (`core/light.rs` 与 docs/24 的约定，
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
mod tests {
    use super::*;
    use crate::core::camera::{Camera2D, Projection, RenderView};
    use crate::core::math3d::Quat;
    use crate::core::transform::Vec2;

    use crate::host::frame::OverlayVertex;

    #[test]
    fn render_metrics_count_2d_draws_and_geometry() {
        let mut metrics = RenderFrameMetrics::default();
        record_vertex_draw(&mut metrics, 60);
        record_vertex_draw(&mut metrics, 30);

        assert_eq!(metrics.draw_calls, 2);
        assert_eq!(metrics.instances, 15);
        assert_eq!(metrics.triangles, 30);
        assert_eq!(metrics.gpu_frame_us, None);
    }

    #[test]
    fn render_metrics_do_not_treat_vector_text_as_sprite_instances() {
        let mut metrics = RenderFrameMetrics::default();
        record_triangle_draw(&mut metrics, 12);

        assert_eq!(metrics.draw_calls, 1);
        assert_eq!(metrics.instances, 0);
        assert_eq!(metrics.triangles, 4);
    }

    /// docs/54 §10 第 ⑩ 行登记过的那条符号缺陷：阴影相机必须放在**光源那一侧**（`-direction`），
    /// 与着色器 `l = normalize(-light.direction)` 同源。
    ///
    /// 判据用"深度"而不是相机位置本身：阴影相机在光源一侧时，**离光源更近**的点（也就是更高、
    /// 更靠上的表面）在光空间里深度更小。光传播方向朝下时，位于地面上方的点必须比地面更接近光源。
    #[test]
    fn shadow_camera_sits_on_the_light_side_of_the_documented_direction() {
        // 天空里的太阳：光传播方向朝下（y < 0）。
        let sun = Light {
            direction: Vec3::new(0.4, -0.8, 0.5),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
        };
        let matrix = light_view_projection(sun);
        let depth = |point: Vec3| -> f32 {
            let clip = matrix
                .transform_point(point)
                .expect("light-space projection of a finite point");
            clip.z
        };
        let ground = depth(Vec3::ZERO);
        let above = depth(Vec3::new(0.0, 1.0, 0.0));
        assert!(
            above < ground,
            "光朝下时，地面上方的点必须更接近阴影相机：above={above} ground={ground}"
        );
        // 反例（曾经的写法）：把相机放到 `+direction` 一侧会让上面的关系反过来——把光方向
        // 反号就应当得到相反的结论，这条同时钉住"符号真的起作用"。
        let flipped = Light {
            direction: -sun.direction,
            ..sun
        };
        let flipped_matrix = light_view_projection(flipped);
        let flipped_clip = flipped_matrix
            .transform_point(Vec3::new(0.0, 1.0, 0.0))
            .expect("clip");
        let flipped_ground = flipped_matrix.transform_point(Vec3::ZERO).expect("clip");
        assert!(
            flipped_clip.z > flipped_ground.z,
            "光方向取反后，上方的点必须变得**更远**：{} vs {}",
            flipped_clip.z,
            flipped_ground.z
        );
    }

    #[test]
    fn render_metrics_count_3d_passes_once_per_instance() {
        let mut metrics = RenderFrameMetrics::default();
        record_indexed_draw(&mut metrics, 36, 4, 2);

        assert_eq!(metrics.draw_calls, 2);
        assert_eq!(metrics.instances, 4);
        assert_eq!(metrics.triangles, 48);
    }

    #[test]
    fn render_metrics_count_mixed_2d_and_3d_frame() {
        let mut metrics = RenderFrameMetrics::default();
        record_indexed_draw(&mut metrics, 12, 3, 2);
        record_vertex_draw(&mut metrics, 18);

        assert_eq!(metrics.draw_calls, 3);
        assert_eq!(metrics.instances, 6);
        assert_eq!(metrics.triangles, 18);
    }

    /// 2D 顶点缓冲容量策略：不小于初始容量、按 2 的幂增长、且总是能容纳请求。
    /// 回归背景：此前容量固定为 1 MiB 且超限时**没有 else 分支**，超出的帧会
    /// 静默丢掉整层 2D 绘制（含 UI 与文字），日志里没有任何线索。
    #[test]
    fn two_d_vertex_capacity_grows_to_powers_of_two() {
        assert_eq!(
            next_2d_vertex_capacity(0),
            INITIAL_2D_VERTEX_BUFFER_BYTES,
            "空帧不应缩小缓冲"
        );
        assert_eq!(
            next_2d_vertex_capacity(INITIAL_2D_VERTEX_BUFFER_BYTES),
            INITIAL_2D_VERTEX_BUFFER_BYTES,
            "正好装满不扩容"
        );
        // 超出一点：直接跳到 2 MiB（而不是 1 MiB + 若干字节），避免逐帧重建。
        assert_eq!(
            next_2d_vertex_capacity(INITIAL_2D_VERTEX_BUFFER_BYTES + 36),
            2 * 1024 * 1024
        );
        assert_eq!(next_2d_vertex_capacity(5 * 1024 * 1024), 8 * 1024 * 1024);
        // 任何请求都必须被容纳（扩容策略的正确性下限）。
        for required in [
            1_u64,
            1024,
            1024 * 1024 - 1,
            1024 * 1024 + 1,
            3 * 1024 * 1024 + 7,
            17 * 1024 * 1024,
        ] {
            let capacity = next_2d_vertex_capacity(required);
            assert!(
                capacity >= required,
                "capacity {capacity} must hold {required} bytes"
            );
            assert!(
                capacity.is_power_of_two(),
                "{capacity} must be a power of two"
            );
        }
    }

    /// Renders an empty frame to an offscreen texture and reads it back,
    /// proving the offscreen path (`render_into_texture` +
    /// `copy_texture_to_cpu`) produces real pixels. Skips when no GPU is
    /// available.
    #[test]
    fn offscreen_renderer_produces_pixels() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-test-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
            .expect("offscreen renderer");
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-test-preview"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mesh = crate::host::frame::SceneMesh {
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
        };
        let outcome = renderer.render_into_texture(&mesh, None, &view, 64, 64);
        assert!(matches!(outcome, RenderOutcome::Presented));
        let pixels = renderer
            .copy_texture_to_cpu(&texture, 64, 64)
            .expect("preview readback");
        assert_eq!(pixels.len(), 64 * 64 * 4);
        // Clear color is (0.04, 0.07, 0.12); the frame must not be all zero.
        assert!(
            pixels.iter().any(|&byte| byte > 0),
            "offscreen frame is blank"
        );
    }

    /// WP6 世界 Sprite 通道的遮挡契约：**测深度但不写深度**，因此顺序即一切。
    /// ① 按 z 排成画家顺序（远→近）时近处胜出；② 顺序反过来时远处胜出——两条一起说明
    /// 深度不会兜底，排序必须由宿主保证（`SpriteBatch::build_world_into` + `sort_world_ranges`）。
    #[test]
    fn world_sprite_pass_is_painter_ordered_without_depth_writes() {
        const SIZE: u32 = 64;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-world-sprite-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
            .expect("offscreen renderer");
        let viewport = Viewport::new(SIZE as f32, SIZE as f32).unwrap();
        // 相机中心 (0,0)、zoom = SIZE（1 世界单位 = SIZE 像素）⇒ 世界 ±0.5 覆盖整个目标。
        let camera = crate::core::camera::Camera2D::new(
            crate::core::transform::Vec2::new(0.0, 0.0),
            SIZE as f32,
        )
        .expect("camera");
        renderer.set_world_sprite_camera(camera.view_projection(viewport, -1.0, 1.0).unwrap());

        let quad = |z: f32, color: [f32; 4]| {
            let mut vertices = Vec::new();
            for (x, y) in [
                (-0.5_f32, -0.5_f32),
                (0.5, -0.5),
                (-0.5, 0.5),
                (0.5, -0.5),
                (0.5, 0.5),
                (-0.5, 0.5),
            ] {
                vertices.push(WorldSpriteVertex {
                    position: [x, y, z],
                    uv: [0.5, 0.5],
                    color,
                });
            }
            vertices
        };
        let mut mesh = SceneMesh {
            vertices: Vec::new(),
            draws: Vec::new(),
            // 画家顺序：先远（红，z=+0.5）、后近（绿，z=-0.5）。
            world_sprites: quad(0.5, [1.0, 0.0, 0.0, 1.0]),
            world_sprite_draws: Vec::new(),
            paths: vec![WHITE_TEXTURE_PATH.to_owned()],
            path_handles: Vec::new(),
            font_atlas: None,
            custom_font_atlases: Vec::new(),
            vector_vertices: Vec::new(),
            vector_glyphs: Vec::new(),
            vector_segments: Vec::new(),
        };
        mesh.world_sprites.extend(quad(-0.5, [0.0, 1.0, 0.0, 1.0]));
        // 纯 2D 回滚档（宿主没有声明任何视图）：世界 Sprite 只有画家顺序，深度策略不参与，
        // 因此这里按"不写深度"构造，与本条测试的断言一一对应。
        mesh.world_sprite_draws = vec![
            crate::core::sprite_batch::SpriteDrawBatch::translucent(
                0,
                0,
                crate::core::layer::RenderLayer::World2D,
                0,
                6,
            ),
            crate::core::sprite_batch::SpriteDrawBatch::translucent(
                0,
                1,
                crate::core::layer::RenderLayer::World2D,
                6,
                6,
            ),
        ];

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-world-sprite-target"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let center_of = |renderer: &mut Renderer, mesh: &SceneMesh, texture: &wgpu::Texture| {
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            renderer.render_into_texture(mesh, None, &view, SIZE, SIZE);
            let pixels = renderer
                .copy_texture_to_cpu(texture, SIZE, SIZE)
                .expect("readback");
            let index = (((SIZE / 2) * SIZE + SIZE / 2) * 4) as usize;
            [
                pixels[index],
                pixels[index + 1],
                pixels[index + 2],
                pixels[index + 3],
            ]
        };

        // ① 画家顺序（远 → 近，宿主排好）：近处的绿色**后画**并盖住红色。这正是
        //    `sort_world_ranges` 保证的顺序，半透明因此可以正常叠加。
        let painted = center_of(&mut renderer, &mesh, &texture);
        assert_eq!(
            painted,
            [0, 255, 0, 255],
            "按 z 排成远→近后，近处的绿色应盖住远处的红色"
        );

        // ② 顺序反过来（近的先画、远的后画）：远处红色**确实**盖住绿色——这条通道
        //    **不写深度**，顺序就是一切。这条把"排序是宿主的责任"钉成契约：哪天有人
        //    以为深度会兜底，这里立刻判红。
        let mut reversed = SceneMesh {
            vertices: Vec::new(),
            draws: Vec::new(),
            // 反过来的顺序：先近（绿，z=-0.5）、后远（红，z=+0.5）。
            world_sprites: quad(-0.5, [0.0, 1.0, 0.0, 1.0]),
            world_sprite_draws: Vec::new(),
            paths: vec![WHITE_TEXTURE_PATH.to_owned()],
            path_handles: Vec::new(),
            font_atlas: None,
            custom_font_atlases: Vec::new(),
            vector_vertices: Vec::new(),
            vector_glyphs: Vec::new(),
            vector_segments: Vec::new(),
        };
        reversed
            .world_sprites
            .extend(quad(0.5, [1.0, 0.0, 0.0, 1.0]));
        reversed.world_sprite_draws = mesh.world_sprite_draws.clone();
        let unpainted = center_of(&mut renderer, &reversed, &texture);
        assert_eq!(
            unpainted,
            [255, 0, 0, 255],
            "不写深度时后画的远处红色会盖住绿色——排序必须由宿主保证"
        );
    }

    /// docs/54 §8-④ 的核心证明：**同一世界视图**里 Sprite 与 Mesh 按深度策略正确互相遮挡。
    ///
    /// 场景只用内建几何（`__ground_plane__` 哨兵），因此不依赖任何示例工程资源：
    /// - 主视图 = 透视相机（`RenderView::main_3d` 携带视图投影矩阵），世界 Sprite 与网格共用它；
    /// - 网格是那块地面四边形，绕 X 转 90° 后正对相机、放在 z = -1；
    /// - 精灵是不透明四边形（alpha = 1 ⇒ 写深度）。
    ///
    /// 两个方向各断言一次：精灵在网格**前面**时精灵胜出；精灵在网格**后面**时网格胜出。
    /// 收敛前这两条都不可能同时成立——精灵有自己的深度附件、且永远在网格之后画。
    #[test]
    fn world_sprites_and_meshes_occlude_each_other_in_one_view() {
        const SIZE: u32 = 64;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-mixed-occlusion-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
            .expect("offscreen renderer");
        let viewport = Viewport::new(SIZE as f32, SIZE as f32).unwrap();
        let camera = Camera3D::perspective(
            Vec3::new(0.0, 0.0, 4.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            0.1,
            100.0,
        )
        .expect("perspective camera");
        // 宿主声明主视图：世界 Sprite 通道因此与网格共用相机、视口与深度附件。
        renderer.set_frame_views(FrameViews {
            main: Some(RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR)),
            overlay: None,
        });

        // 地面四边形：绕 X 转 90° 后法线朝 +Z，落在 z = -1 的平面上，尺寸 ±4。
        let facing =
            Mat4::from_quat(Quat::from_axis_angle(Vec3::X, std::f32::consts::FRAC_PI_2).unwrap());
        let mesh_model = Mat4::translation(Vec3::new(0.0, 0.0, -1.0)) * facing;
        let scene = Scene3d {
            camera,
            view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
            light: Light::default(),
            drawables: vec![crate::host::frame::Scene3dDrawable {
                entity: crate::core::scene::SceneEntityId(1),
                name: "Ground".to_owned(),
                mesh_path: GROUND_PLANE_SENTINEL.to_owned(),
                // 测试手写场景：给一个本地句柄即可（渲染器只把它当缓存键）。
                mesh_handle: Some(AssetHandle::new(0, 1)),
                model: mesh_model,
                palette: Vec::new(),
                bounds: crate::host::frame::Scene3dBounds {
                    local_aabb: crate::core::math3d::Aabb3 {
                        min: Vec3::new(-4.0, -4.0, -0.1),
                        max: Vec3::new(4.0, 4.0, 0.1),
                    },
                    source: crate::host::frame::BoundsSource::Fallback,
                },
            }],
        };

        let quad = |z: f32| {
            let mut vertices = Vec::new();
            for (x, y) in [
                (-0.5_f32, -0.5_f32),
                (0.5, -0.5),
                (-0.5, 0.5),
                (0.5, -0.5),
                (0.5, 0.5),
                (-0.5, 0.5),
            ] {
                vertices.push(WorldSpriteVertex {
                    position: [x, y, z],
                    uv: [0.5, 0.5],
                    color: [1.0, 0.0, 0.0, 1.0],
                });
            }
            vertices
        };
        let mesh_with_sprite = |z: f32| SceneMesh {
            vertices: Vec::new(),
            draws: Vec::new(),
            world_sprites: quad(z),
            // 不透明（alpha = 1）⇒ 这一段落写深度，因此能与网格互相遮挡。
            world_sprite_draws: vec![crate::core::sprite_batch::SpriteDrawBatch::opaque(
                0,
                0,
                crate::core::layer::RenderLayer::World2D,
                0,
                6,
            )],
            paths: vec![WHITE_TEXTURE_PATH.to_owned()],
            path_handles: Vec::new(),
            font_atlas: None,
            custom_font_atlases: Vec::new(),
            vector_vertices: Vec::new(),
            vector_glyphs: Vec::new(),
            vector_segments: Vec::new(),
        };

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-mixed-occlusion-target"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let center_of = |renderer: &mut Renderer, mesh: &SceneMesh| {
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            renderer.render_into_texture(mesh, Some(&scene), &view, SIZE, SIZE);
            let pixels = renderer
                .copy_texture_to_cpu(&target, SIZE, SIZE)
                .expect("readback");
            let index = (((SIZE / 2) * SIZE + SIZE / 2) * 4) as usize;
            [
                pixels[index],
                pixels[index + 1],
                pixels[index + 2],
                pixels[index + 3],
            ]
        };
        let is_sprite_red = |pixel: [u8; 4]| pixel[0] > 128 && pixel[1] < 96 && pixel[2] < 96;

        // ① 精灵在网格前面（z = 0 > -1）：精灵胜出。
        let in_front = center_of(&mut renderer, &mesh_with_sprite(0.0));
        assert!(
            is_sprite_red(in_front),
            "网格前面的不透明精灵必须可见，实际 {in_front:?}"
        );

        // ② 精灵在网格后面（z = -2 < -1）：网格胜出 ⇒ 网格**能遮挡**精灵。
        let behind = center_of(&mut renderer, &mesh_with_sprite(-2.0));
        assert!(
            !is_sprite_red(behind),
            "网格后面的精灵必须被网格遮住，实际 {behind:?}"
        );

        // ③ 半透明精灵即使在前也不写深度：它不会反过来遮住网格的深度（策略按分段下发）。
        let mut translucent = mesh_with_sprite(0.0);
        translucent.world_sprite_draws =
            vec![crate::core::sprite_batch::SpriteDrawBatch::translucent(
                0,
                0,
                crate::core::layer::RenderLayer::World2D,
                0,
                6,
            )];
        for vertex in &mut translucent.world_sprites {
            vertex.color = [1.0, 0.0, 0.0, 0.5];
        }
        let blended = center_of(&mut renderer, &translucent);
        assert!(
            blended != behind,
            "半透明精灵应与'被遮挡'是两种不同的画面（说明它确实画上去了）"
        );
    }

    /// WP6：三条管线的深度策略逐条钉住。
    ///
    /// 回归背景：这三个设置曾是散落的字段字面量，一次脚本化编辑把 **mesh3d 与 shadow** 的
    /// `depth_write_enabled` 误改成 `false`——3D 深度静默失效，而当时的 golden 只断言可绘制
    /// 对象、实例/三角形与非背景像素，**断言不了深度顺序**，因此全门绿灯。现在策略收在
    /// `DepthPass` 里并由本条测试锁死；要改必须先改这里，改错立刻判红。
    #[test]
    fn depth_policies_are_pinned_per_pass() {
        assert!(
            DepthPass::Mesh3d.write_depth(),
            "3D 网格必须写深度：否则前后遮挡不再生效（这正是被误改过的那一处）"
        );
        assert!(
            DepthPass::Shadow.write_depth(),
            "阴影贴图必须写深度：它本身就是不透明物体到光源的距离场"
        );
        assert!(
            !DepthPass::WorldSprite.write_depth(),
            "半透明世界 Sprite 不写深度：顺序由宿主按 z 排成画家顺序，写深度会让半透明透不出来"
        );
        assert!(
            DepthPass::WorldSpriteOpaque.write_depth(),
            "不透明世界 Sprite 必须写深度：否则它无法与 3D 网格在同一世界视图里互相遮挡"
        );
        for pass in [
            DepthPass::Mesh3d,
            DepthPass::Shadow,
            DepthPass::WorldSprite,
            DepthPass::WorldSpriteOpaque,
        ] {
            assert_eq!(
                depth_stencil_state(pass, wgpu::DepthBiasState::default()).depth_compare,
                wgpu::CompareFunction::LessEqual,
                "{pass:?} 的深度比较应为 LessEqual"
            );
        }
    }

    /// docs/54 §8-④：世界 Sprite 到底用哪个相机、哪个深度附件，必须由**声明的视图**决定，
    /// 而不是由"谁最后调了哪个 setter"决定。这条把三种情形逐条钉住（无需 GPU）。
    #[test]
    fn world_sprite_view_is_resolved_from_the_declared_views() {
        let viewport = Viewport::new(640.0, 360.0).unwrap();
        let half = Viewport::new(640.0, 180.0).unwrap();
        let camera3d = Camera3D::perspective(
            Vec3::new(0.0, 0.0, 6.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            0.1,
            100.0,
        )
        .unwrap();
        let camera2d =
            crate::core::camera::Camera2D::new(crate::core::transform::Vec2::zero(), 1.0).unwrap();
        let main = RenderView::main_3d(camera3d, viewport, DEFAULT_3D_CLEAR);
        let overlay = RenderView::overlay_2d(camera2d, half);
        let explicit = Mat4::identity();

        // ① 声明了主视图：与主视图共用相机、视口与深度（这才是"同一世界视图"）。
        let shared =
            resolve_world_sprite_view(Some(main), Some(overlay), main.matrix(), explicit, None);
        assert!(
            shared.shared_depth,
            "声明主视图时世界 Sprite 必须共用深度附件"
        );
        assert_eq!(
            shared.viewport,
            Some(viewport),
            "世界 Sprite 用主视图的视口"
        );
        assert_eq!(
            shared.matrix,
            main.matrix().expect("主 3D 视图应带视图投影矩阵"),
            "世界 Sprite 与网格必须是同一份投影矩阵"
        );

        // ② 只有 overlay（纯 2D 画面）：用 overlay 的世界相机，但深度附件独立。
        let isolated =
            resolve_world_sprite_view(None, Some(overlay), None, explicit, Some(viewport));
        assert!(!isolated.shared_depth, "纯 2D 画面没有网格可共享深度");
        assert_eq!(isolated.viewport, Some(half));
        assert_eq!(isolated.matrix, overlay.matrix().unwrap());

        // ③ 什么都没声明（回滚档）：用宿主显式设置的矩阵与传入的默认视口。
        let legacy = resolve_world_sprite_view(None, None, None, explicit, Some(viewport));
        assert!(!legacy.shared_depth);
        assert_eq!(legacy.matrix, explicit);
        assert_eq!(legacy.viewport, Some(viewport));
    }

    /// 主视图**没有**矩阵（退化相机 / 手工构造的字面量）时，世界 Sprite 退回显式矩阵而不是
    /// 悄悄用一份单位阵——错的是取景，不该表现为"精灵全在屏幕中央"。
    #[test]
    fn world_sprite_view_falls_back_to_the_explicit_matrix_without_a_view_matrix() {
        let viewport = Viewport::new(320.0, 240.0).unwrap();
        let matrixless = RenderView::degenerate(
            Projection::Orthographic { zoom: 2.0 },
            viewport,
            ClearPolicy::None,
            0,
            u32::MAX,
        );
        let explicit = Mat4::translation(Vec3::new(3.0, 4.0, 5.0));
        let resolved = resolve_world_sprite_view(Some(matrixless), None, None, explicit, None);
        assert_eq!(resolved.matrix, explicit);
        assert!(resolved.shared_depth);
    }
    /// WP5 视口夹取：声明的视口必须落在目标内、至少 1×1，且整目标视口与"未声明"等价。
    #[test]
    fn clamp_viewport_keeps_rects_inside_the_target() {
        let full = Viewport::new(64.0, 64.0).unwrap();
        assert_eq!(clamp_viewport(full, (64, 64)), (0, 0, 64, 64));
        // 超出目标：夹到目标尺寸，而不是让 wgpu 校验报错。
        assert_eq!(clamp_viewport(full, (32, 16)), (0, 0, 32, 16));
        // 逻辑像素取整：59.6 → 60。
        let rounded = Viewport::new(59.6, 10.2).unwrap();
        assert_eq!(clamp_viewport(rounded, (64, 64)), (0, 0, 60, 10));
        // 空/非法尺寸（构造器会拒绝，这里兜住手写的 0）：至少 1×1。
        let zero = Viewport::new(1.0, 1.0).unwrap();
        assert_eq!(clamp_viewport(zero, (0, 0)), (0, 0, 1, 1));
    }

    /// WP5：渲染器消费宿主声明的 [`crate::core::camera::FrameViews`]——清屏色来自声明，且 2D overlay 的视口
    /// 真的只画在声明的那块区域内。
    ///
    /// 同时这是**收敛的对拍**：不声明任何视图时读回的第一像素，必须与"声明历史默认
    /// 清屏色"的帧逐字节相同（回滚档与接线后行为一致）。
    #[test]
    fn declared_frame_views_own_clear_color_and_overlay_viewport() {
        const SIZE: u32 = 64;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-view-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
            .expect("offscreen renderer");
        let empty = crate::host::frame::SceneMesh {
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
        };
        let render_and_read = |renderer: &mut Renderer, target: &wgpu::Texture| {
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            renderer.render_into_texture(&empty, None, &view, SIZE, SIZE);
            renderer
                .copy_texture_to_cpu(target, SIZE, SIZE)
                .expect("readback")
        };
        let new_target = |label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let pixel = |pixels: &[u8], x: u32, y: u32| -> [u8; 4] {
            let offset = ((y * SIZE + x) * 4) as usize;
            [
                pixels[offset],
                pixels[offset + 1],
                pixels[offset + 2],
                pixels[offset + 3],
            ]
        };
        let viewport = Viewport::new(SIZE as f32, SIZE as f32).unwrap();

        // 1) 不声明任何视图（回滚档）：读回历史默认清屏色。
        let baseline = render_and_read(&mut renderer, &new_target("rustlua-view-baseline"));
        let clear = pixel(&baseline, 0, 0);
        assert!(
            clear.iter().any(|&byte| byte > 0),
            "默认清屏应产生非零像素：{clear:?}"
        );

        // 2) 声明主视图但用**历史默认色**：必须与回滚档逐字节相同。
        let declared_default = FrameViews {
            main: Some(RenderView::degenerate(
                Projection::Perspective {
                    fov_y: 1.0,
                    near: 0.1,
                    far: 100.0,
                },
                viewport,
                ClearPolicy::Color(DEFAULT_3D_CLEAR),
                0,
                u32::MAX,
            )),
            overlay: None,
        };
        renderer.set_frame_views(declared_default);
        let unplugged =
            render_and_read(&mut renderer, &new_target("rustlua-view-declared-default"));
        assert_eq!(
            unplugged, baseline,
            "声明历史默认色与不声明必须是同一帧（回滚档逐字节等价）"
        );

        // 3) 声明一个不同的清屏色：整帧都必须是该颜色 ⇒ 清屏确实由声明驱动。
        renderer.set_frame_views(FrameViews {
            main: Some(
                declared_default
                    .main
                    .expect("main view")
                    .with_clear(ClearPolicy::Color([1.0, 0.0, 0.0, 1.0])),
            ),
            overlay: None,
        });
        let red_target = new_target("rustlua-view-red");
        let red = render_and_read(&mut renderer, &red_target);
        assert_eq!(
            pixel(&red, 0, 0),
            [255, 0, 0, 255],
            "主视图声明的清屏色必须生效"
        );
        assert_eq!(pixel(&red, SIZE - 1, SIZE - 1), [255, 0, 0, 255]);

        // 4) 声明"不清屏"：**同一张附件**上再画一帧空场景，红色必须原样留着
        //    （说明这一帧真的没有清屏，而不是又清成了别的颜色）。
        renderer.set_frame_views(FrameViews {
            main: Some(
                declared_default
                    .main
                    .expect("main view")
                    .with_clear(ClearPolicy::None),
            ),
            overlay: None,
        });
        let kept = render_and_read(&mut renderer, &red_target);
        assert_eq!(
            pixel(&kept, 0, 0),
            [255, 0, 0, 255],
            "声明 ClearPolicy::None 时不应清屏"
        );

        // 5) 2D overlay 视口：只画左半屏（32×64），白 quad 只应覆盖左半。
        let half = Viewport::new((SIZE / 2) as f32, SIZE as f32).unwrap();
        let quad = crate::host::frame::SceneMesh {
            vertices: vec![
                OverlayVertex {
                    position: [-1.0, -1.0],
                    uv: [0.0, 1.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [1.0, -1.0],
                    uv: [1.0, 1.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [-1.0, 1.0],
                    uv: [0.0, 0.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [1.0, -1.0],
                    uv: [1.0, 1.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [1.0, 1.0],
                    uv: [1.0, 0.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [-1.0, 1.0],
                    uv: [0.0, 0.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                    sdf: 0,
                },
            ],
            draws: vec![crate::host::frame::SceneDraw {
                texture: 0,
                start: 0,
                count: 6,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: crate::core::layer::RenderLayer::World2D,
            }],
            world_sprites: Vec::new(),
            world_sprite_draws: Vec::new(),
            paths: Vec::new(),
            path_handles: Vec::new(),
            font_atlas: None,
            custom_font_atlases: Vec::new(),
            vector_vertices: Vec::new(),
            vector_glyphs: Vec::new(),
            vector_segments: Vec::new(),
        };
        renderer.set_frame_views(FrameViews {
            main: Some(
                declared_default
                    .main
                    .expect("main view")
                    .with_clear(ClearPolicy::Color(DEFAULT_3D_CLEAR)),
            ),
            overlay: Some(RenderView::degenerate(
                Projection::Orthographic { zoom: 1.0 },
                half,
                ClearPolicy::None,
                1,
                u32::MAX,
            )),
        });
        let clipped = {
            let target = new_target("rustlua-view-overlay");
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            renderer.render_into_texture(&quad, None, &view, SIZE, SIZE);
            renderer
                .copy_texture_to_cpu(&target, SIZE, SIZE)
                .expect("readback")
        };
        assert_eq!(
            pixel(&clipped, 4, SIZE / 2),
            [255, 255, 255, 255],
            "声明的 overlay 视口内应画出 quad"
        );
        assert_eq!(
            pixel(&clipped, SIZE - 5, SIZE / 2),
            clear,
            "视口之外的右半屏必须保持清屏色（视口确实生效）"
        );
    }

    /// 俄罗斯方块离屏渲染：方块（white.png 精灵）应实际可见——I 方块起始
    /// 位于 col 4-7/row 0，其红色调应出现在字段区域的像素上。
    #[test]
    fn tetris_blocks_are_visible_pixels() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-tetris-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let root = crate::core::fixture_games_dir().join("tetris");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let mut renderer = Renderer::new_offscreen(
            device.clone(),
            queue.clone(),
            Some(root.clone()),
            false,
            false,
        )
        .expect("offscreen renderer");
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-tetris-preview"),
            size: wgpu::Extent3d {
                width: 1280,
                height: 720,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let scene = crate::core::scene::Scene::load(root.join("scenes/main.scene")).unwrap();
        let mut runtime = crate::host::RuntimeScene::from_scene(Some(&root), scene).unwrap();
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        renderer.render_into_texture(&mesh, None, &view, 1280, 720);
        let pixels = renderer
            .copy_texture_to_cpu(&texture, 1280, 720)
            .expect("readback");
        // 扫描全帧找 I 方块（红色调 r 显著大于 g）的可见像素。
        let mut found = 0usize;
        let mut first = (0usize, 0usize);
        for y in 0..720usize {
            for x in 0..1280usize {
                let idx = (y * 1280 + x) * 4;
                let (r, g, b) = (pixels[idx], pixels[idx + 1], pixels[idx + 2]);
                // I 色 (1,0.45,0.45) 线性 → sRGB (255,179,179)：红主导即方块。
                let _ = b;
                if r > 200 && i32::from(r) > i32::from(g) {
                    if found == 0 {
                        first = (x, y);
                    }
                    found += 1;
                }
            }
        }
        assert!(
            found > 0,
            "未找到红色方块像素（I 方块应可见），首像素位置样本：{}x{}",
            first.0,
            first.1
        );
        // 首块 I 方块 cell(5,0) 世界 (655,75) 附近应能看到红色。
        eprintln!("red block pixels={found} first at {}x{}", first.0, first.1);
    }

    /// 重开（R）应清屏：初始 4 块 → 硬降后 8 块 → 重开后只剩新方块 4 块。
    #[test]
    fn tetris_restart_clears_field() {
        use crate::core::input::{InputSnapshot, KeyCode};
        use crate::core::scene::Scene;

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-tetris-restart-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("device");
        let root = crate::core::fixture_games_dir().join("tetris");
        let mut renderer = Renderer::new_offscreen(
            device.clone(),
            queue.clone(),
            Some(root.clone()),
            false,
            false,
        )
        .expect("offscreen renderer");
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-tetris-restart"),
            size: wgpu::Extent3d {
                width: 1280,
                height: 720,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let root = crate::core::fixture_games_dir().join("tetris");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let scene = Scene::load(root.join("scenes/main.scene")).unwrap();
        let mut runtime = crate::host::RuntimeScene::from_scene(Some(&root), scene).unwrap();
        // 统计字段区域（屏幕 486..794 × 56..664）内的亮块像素（方块）。
        let mut count_blocks = |runtime: &mut crate::host::RuntimeScene| -> usize {
            let mesh = runtime.vertices(1280, 720).unwrap();
            renderer.render_into_texture(&mesh, None, &view, 1280, 720);
            let pixels = renderer.copy_texture_to_cpu(&texture, 1280, 720).unwrap();
            let mut n = 0;
            for y in 56..664usize {
                for x in 486..794usize {
                    let idx = (y * 1280 + x) * 4;
                    let m = pixels[idx].max(pixels[idx + 1]).max(pixels[idx + 2]);
                    if m > 150 {
                        n += 1;
                    }
                }
            }
            n
        };
        let initial = count_blocks(&mut runtime);
        assert_eq!(initial, 4 * 28 * 28, "初始应是 4 块，got {initial}");
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::Space, true); // 硬降：放置当前块 + 生成新块
        runtime.run_script_steps(1, 1.0 / 60.0, &input).unwrap();
        let after_drop = count_blocks(&mut runtime);
        assert_eq!(
            after_drop,
            8 * 28 * 28,
            "硬降后应 8 块（4 放置 + 4 新），got {after_drop}"
        );
        input.key_event(KeyCode::Space, false);
        input.key_event(
            KeyCode::from_char('r').expect("QWERTY 字符必须有对应物理键"),
            true,
        ); // 重开
        runtime.run_script_steps(1, 1.0 / 60.0, &input).unwrap();
        let after_restart = count_blocks(&mut runtime);
        assert_eq!(
            after_restart,
            4 * 28 * 28,
            "重开应清屏只剩 4 块，got {after_restart}"
        );
    }

    /// Non-blocking readback (`readback_frame`) eventually yields the rendered
    /// pixels without a blocking `device.poll(Wait)` on the first call. Skips
    /// when no GPU is available.
    #[test]
    fn non_blocking_readback_eventually_yields_pixels() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-test-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
            .expect("offscreen renderer");
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-test-readback"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mesh = crate::host::frame::SceneMesh {
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
        };
        renderer.render_into_texture(&mesh, None, &view, 64, 64);
        // 首帧提交；随后的调用非阻塞轮询，直到返回就绪像素（不应无限阻塞）。
        let mut result = renderer.readback_frame(&texture, 64, 64);
        for _ in 0..120 {
            match &result {
                ReadbackStatus::Ready {
                    pixels,
                    width,
                    height,
                } => {
                    assert_eq!((*width, *height), (64, 64));
                    assert_eq!(pixels.len(), 64 * 64 * 4);
                    assert!(
                        pixels.iter().any(|&byte| byte > 0),
                        "offscreen frame is blank"
                    );
                    return;
                }
                ReadbackStatus::Pending => {
                    // 未就绪：等 GPU 完成拷贝（真实编辑器帧间隔远大于此），再轮询。
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    result = renderer.readback_frame(&texture, 64, 64);
                }
                ReadbackStatus::Failed(error) => panic!("readback failed: {error}"),
            }
        }
        panic!("readback never became ready within 120 polls");
    }

    /// M11-03（R-04）：映射失败的 readback 槽上报 `Failed` 而非当作就绪返回脏数据，
    /// 且失败槽被清空，后续轮询可重试（不再永久卡在旧状态）。
    #[test]
    fn failed_readback_mapping_reports_error_and_clears_slot() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-test-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer =
            Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
                .expect("offscreen renderer");
        // 提交一个空 encoder 拿到合法的 SubmissionIndex（无需真实回读流程）。
        let empty = device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default())
            .finish();
        let submission = queue.submit([empty]);
        // 注入一个"映射失败"的槽（模拟 map_async 回调返回 Err）。
        renderer.readback_slots.push(ReadbackSlot {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-test-readback-failed"),
                size: 256 * 8,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            submission: Some(submission),
            map_result: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(2)),
            map_error: std::sync::Arc::new(std::sync::Mutex::new(Some(
                "注入的映射失败".to_owned(),
            ))),
            width: 8,
            height: 8,
            row_bytes: 32,
            bytes_per_row: 256,
            pixels: Vec::new(),
            generation: 0,
        });
        match renderer.readback_pending() {
            ReadbackStatus::Failed(error) => assert!(error.contains("注入的映射失败")),
            _ => panic!("映射失败应上报 Failed 而非脏数据"),
        }
        // 失败槽已清空：再次轮询进入 NotReady → Pending，不再重复报错。
        assert!(matches!(
            renderer.readback_pending(),
            ReadbackStatus::Pending
        ));
        assert!(!renderer.readback_in_flight(), "失败槽应被释放以便重试");
    }

    /// Reproduces the editor's `create_preview` path against the real game
    /// project: instantiate `main.scene`, step, build frames, render offscreen
    /// and read back. Requires the game project beside the engine workspace.
    #[test]
    fn editor_preview_path_renders_game_scene() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            panic!("无法获取用于编辑器预览测试的 wgpu adapter");
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-test-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
            .expect("offscreen renderer");
        let game = crate::core::games_dir().join("examples").join("game_api");
        if !game.join("scenes/main.scene").is_file() {
            return;
        }
        let scene_text =
            std::fs::read_to_string(game.join("scenes/main.scene")).expect("read main.scene");
        let scene = crate::core::scene::Scene::from_text(&scene_text).expect("parse main.scene");
        let mut runtime = crate::host::runtime_scene::RuntimeScene::from_scene(Some(&game), scene)
            .expect("instantiate runtime");
        runtime
            .run_script_steps(1, 1.0 / 60.0, &crate::core::input::InputSnapshot::default())
            .expect("step runtime");
        let mesh = runtime.vertices(640, 360).expect("build 2D mesh");
        let scene3d = runtime.scene_3d(Viewport::new(640.0, 360.0).unwrap());
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-test-preview"),
            size: wgpu::Extent3d {
                width: 640,
                height: 360,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        renderer.render_into_texture(&mesh, scene3d.as_ref(), &view, 640, 360);
        let pixels = renderer
            .copy_texture_to_cpu(&texture, 640, 360)
            .expect("readback");
        assert_eq!(pixels.len(), 640 * 360 * 4);
        assert!(
            pixels.iter().any(|&byte| byte > 0),
            "preview frame is blank"
        );
    }

    /// docs/54 §4.4：贴图解析走**资源身份**（句柄）而不是每 draw 一次线性路径扫描。
    ///
    /// 判据是三条可观测事实：第一帧按句柄未命中（首次加载）、第二帧同句柄命中（O(1) 复用）、
    /// 同一资源**只加载一次**。宿主手写 `SceneMesh`（没有身份）时仍然按路径解析，行为不变。
    #[test]
    fn texture_identity_cache_is_hit_on_the_second_frame() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-test-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let root = crate::core::games_dir().join("examples").join("game_api");
        let texture_path = "assets/textures/player.png";
        if !root.join(texture_path).is_file() {
            return; // 没有示例资源就跳过（与"没有 GPU 就跳过"同一策略）
        }
        let mut renderer =
            Renderer::new_offscreen(device.clone(), queue.clone(), Some(root), false, false)
                .expect("offscreen renderer");
        let size = 32u32;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-test-identity"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());

        // 宿主给出身份（运行时提取就是这条路径）。
        let quad = |handle: AssetHandle| SceneMesh {
            vertices: vec![
                OverlayVertex {
                    position: [-1.0, -1.0],
                    uv: [0.0, 1.0],
                    color: [1.0; 4],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [1.0, -1.0],
                    uv: [1.0, 0.0],
                    color: [1.0; 4],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [-1.0, 1.0],
                    uv: [0.0, 0.0],
                    color: [1.0; 4],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [1.0, -1.0],
                    uv: [1.0, 0.0],
                    color: [1.0; 4],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [1.0, 1.0],
                    uv: [1.0, 0.0],
                    color: [1.0; 4],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [-1.0, 1.0],
                    uv: [0.0, 0.0],
                    color: [1.0; 4],
                    sdf: 0,
                },
            ],
            draws: vec![crate::host::frame::SceneDraw::world(1, 0, 6)],
            world_sprites: Vec::new(),
            world_sprite_draws: Vec::new(),
            paths: vec![texture_path.to_owned()],
            path_handles: vec![handle],
            font_atlas: None,
            custom_font_atlases: Vec::new(),
            vector_vertices: Vec::new(),
            vector_glyphs: Vec::new(),
            vector_segments: Vec::new(),
        };

        let handle = AssetHandle::new(0, 1);
        renderer.render_into_texture(&quad(handle), None, &view, size, size);
        assert_eq!(
            renderer.texture_identity_stats(),
            (0, 1),
            "第一帧句柄还没进缓存：应记一次未命中"
        );
        assert_eq!(renderer.loaded_texture_count(), 1, "同一资源只应加载一次");
        renderer.render_into_texture(&quad(handle), None, &view, size, size);
        assert_eq!(
            renderer.texture_identity_stats(),
            (1, 1),
            "第二帧同句柄必须命中缓存（O(1) 复用，不再扫路径）"
        );
        assert_eq!(
            renderer.loaded_texture_count(),
            1,
            "命中缓存不该重复加载同一资源"
        );

        // 没有身份（宿主手写网格）时走旧路径：仍能画，但句柄统计不动。
        let mut legacy = quad(handle);
        legacy.path_handles.clear();
        renderer.render_into_texture(&legacy, None, &view, size, size);
        assert_eq!(
            renderer.texture_identity_stats(),
            (1, 1),
            "没有身份时按路径解析，身份统计不应变化"
        );
        assert_eq!(renderer.loaded_texture_count(), 1);
    }

    /// docs/54 §4.4：网格解析同样走**资源身份**——同一句柄第二帧命中缓存（O(1)），同一网格只
    /// 解析一次；宿主没给身份（编辑器预览/测试夹具）时按路径解析，行为与收敛前一致。
    #[test]
    fn mesh_identity_cache_is_hit_on_the_second_frame() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-test-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer =
            Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
                .expect("offscreen renderer");
        let size = 32u32;
        let camera = Camera3D::perspective(
            Vec3::new(0.0, 0.0, 4.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            0.1,
            100.0,
        )
        .expect("perspective camera");
        // 夹具自己拼 `Scene3d`，因此也要自己声明视口——视图与相机在这儿成帧一次。
        let viewport = Viewport::new(size as f32, size as f32).expect("viewport");
        let handle = AssetHandle::new(7, 3);
        let scene = |handle: Option<AssetHandle>| Scene3d {
            camera,
            view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
            light: Light::default(),
            drawables: vec![crate::host::frame::Scene3dDrawable {
                entity: crate::core::scene::SceneEntityId(1),
                name: "Ground".to_owned(),
                mesh_path: GROUND_PLANE_SENTINEL.to_owned(),
                mesh_handle: handle,
                model: Mat4::translation(Vec3::new(0.0, 0.0, -1.0)),
                palette: Vec::new(),
                bounds: crate::host::frame::Scene3dBounds {
                    local_aabb: crate::core::math3d::Aabb3 {
                        min: Vec3::new(-4.0, -4.0, -0.1),
                        max: Vec3::new(4.0, 4.0, 0.1),
                    },
                    source: crate::host::frame::BoundsSource::Fallback,
                },
            }],
        };
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-test-mesh-identity"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mesh = crate::host::frame::SceneMesh {
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
        };

        renderer.render_into_texture(&mesh, Some(&scene(Some(handle))), &view, size, size);
        assert_eq!(
            renderer.mesh_identity_stats(),
            (0, 1),
            "第一帧句柄还没进缓存：记一次未命中"
        );
        assert_eq!(renderer.cached_mesh_count(), 1, "同一网格只解析一次");
        renderer.render_into_texture(&mesh, Some(&scene(Some(handle))), &view, size, size);
        assert_eq!(
            renderer.mesh_identity_stats(),
            (1, 1),
            "第二帧同句柄必须命中网格缓存"
        );
        assert_eq!(renderer.cached_mesh_count(), 1, "命中不该重复解析");

        // 没有身份：按路径解析（旧路径），身份统计不应变化。
        renderer.render_into_texture(&mesh, Some(&scene(None)), &view, size, size);
        assert_eq!(
            renderer.mesh_identity_stats(),
            (1, 1),
            "宿主没给身份时不该动身份统计"
        );
        assert_eq!(renderer.cached_mesh_count(), 1);
    }

    /// docs/54 §4.3：**计划就是实际执行的那一份**。    ///
    /// 单测（`core::render_plan`）只证明计划函数本身的顺序与取舍；这条测试在真实（离屏）渲染
    /// 帧上证明渲染器跑的就是计划里列的通道，一条不多一条不少。此前"这一帧跑了哪些通道"只能
    /// 靠读渲染循环的 `if` 猜——现在可以直接断言，而且漏跑会立刻在这里失败。
    #[test]
    fn render_plan_matches_the_executed_passes_on_a_real_frame() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-test-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer =
            Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
                .expect("offscreen renderer");
        let size = 64u32;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-test-plan"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let empty_mesh = || crate::host::frame::SceneMesh {
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
        };
        // 空帧 + 未声明视图 ⇒ 只有回滚档的清屏（历史默认色）。
        renderer.render_into_texture(&empty_mesh(), None, &view, size, size);
        assert_eq!(
            renderer.last_frame_plan().passes(),
            [RenderPass::Clear],
            "空帧 + 未声明视图只应清屏"
        );
        assert_eq!(
            renderer.last_frame_passes(),
            renderer.last_frame_plan().passes(),
            "空帧：执行必须与计划逐条一致"
        );

        // 声明 `ClearPolicy::None` ⇒ 计划里连清屏都不该有（清屏也是一条被计划管着的通道）。
        let declared = RenderView::main_2d(
            Camera2D::new(Vec2::new(size as f32 / 2.0, size as f32 / 2.0), 1.0).expect("camera2d"),
            Viewport::new(size as f32, size as f32).expect("viewport"),
            [0.0, 0.0, 0.0, 1.0],
        );
        renderer.set_frame_views(FrameViews {
            main: Some(declared.with_clear(ClearPolicy::None)),
            overlay: Some(declared),
        });
        let quad = crate::host::frame::SceneMesh {
            vertices: vec![
                OverlayVertex {
                    position: [-1.0, -1.0],
                    uv: [0.0, 1.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [1.0, -1.0],
                    uv: [1.0, 1.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [-1.0, 1.0],
                    uv: [0.0, 0.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [1.0, -1.0],
                    uv: [1.0, 1.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [1.0, 1.0],
                    uv: [1.0, 0.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                    sdf: 0,
                },
                OverlayVertex {
                    position: [-1.0, 1.0],
                    uv: [0.0, 0.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                    sdf: 0,
                },
            ],
            draws: vec![
                crate::host::frame::SceneDraw {
                    texture: 0,
                    start: 0,
                    count: 6,
                    sdf: false,
                    bitmap: false,
                    vector: false,
                    render_layer: RenderLayer::World2D,
                },
                crate::host::frame::SceneDraw::screen_ui(0, 0, 6),
            ],
            ..empty_mesh()
        };
        renderer.render_into_texture(&quad, None, &view, size, size);
        assert_eq!(
            renderer.last_frame_plan().passes(),
            [RenderPass::Legacy2DOverlay, RenderPass::ScreenUI],
            "世界层与屏幕 UI 是两条独立通道，且不清屏时不该有 Clear"
        );
        assert_eq!(
            renderer.last_frame_passes(),
            renderer.last_frame_plan().passes(),
            "2D 帧：执行必须与计划逐条一致"
        );
        assert_eq!(
            renderer.last_frame_metrics().draw_calls,
            2,
            "两条通道各应产出一个 draw"
        );
    }

    /// docs/54 §4.3：后处理通道写像素的**方向性**证据。
    ///
    /// 恒等参数不该把画面弄花（离屏 sRGB 往返最多差几个 LSB），而曝光、色调与暗角三个参数各自
    /// 必须产生**可判定方向**的变化——否则"通道跑了"与"通道真的生效"是两件事。
    #[test]
    fn post_process_pass_changes_pixels_in_the_documented_way() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-test-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer =
            Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
                .expect("offscreen renderer");
        let size = 32u32;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-test-post-process"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mesh = crate::host::frame::SceneMesh {
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
        };
        let render_and_read = |renderer: &mut Renderer| -> Vec<u8> {
            renderer.render_into_texture(&mesh, None, &view, size, size);
            renderer
                .copy_texture_to_cpu(&texture, size, size)
                .expect("post-process readback")
        };
        let pixel = |pixels: &[u8], x: u32, y: u32| -> [u8; 4] {
            let offset = ((y * size + x) * 4) as usize;
            [
                pixels[offset],
                pixels[offset + 1],
                pixels[offset + 2],
                pixels[offset + 3],
            ]
        };

        // 回滚档：不开后处理时，计划里没有 PostProcess，整条离屏路径不存在。
        let baseline = render_and_read(&mut renderer);
        assert!(
            !renderer.last_frame_plan().contains(RenderPass::PostProcess),
            "没开后处理时计划里不该有 PostProcess"
        );
        assert_eq!(
            renderer.last_frame_passes(),
            renderer.last_frame_plan().passes()
        );

        // 恒等参数：画面必须基本不变（离屏 sRGB 往返允许多个 LSB 的量化差）。
        renderer.set_post_process(Some(PostProcess::default()));
        let identity = render_and_read(&mut renderer);
        assert!(
            renderer.last_frame_plan().contains(RenderPass::PostProcess),
            "开了后处理，计划必须多出 PostProcess"
        );
        assert_eq!(
            renderer.last_frame_passes(),
            renderer.last_frame_plan().passes()
        );
        assert_eq!(
            renderer.last_frame_passes().last(),
            Some(&RenderPass::PostProcess),
            "后处理永远是最后一条通道"
        );
        for (index, (before, after)) in baseline.iter().zip(&identity).enumerate() {
            assert!(
                before.abs_diff(*after) <= 2,
                "恒等参数不该改变像素（第 {index} 字节：{before} → {after}）"
            );
        }

        // 曝光 0.5：整体压暗。
        renderer.set_post_process(Some(
            PostProcess::new(0.5, [1.0; 4], 0.0).expect("exposure"),
        ));
        let darker = render_and_read(&mut renderer);
        let centre_before = pixel(&identity, size / 2, size / 2);
        let centre_darker = pixel(&darker, size / 2, size / 2);
        assert!(
            centre_darker[0] < centre_before[0] && centre_darker[2] < centre_before[2],
            "曝光 0.5 必须压暗：{centre_before:?} → {centre_darker:?}"
        );

        // 色调（只留红）：绿/蓝分量必须被压到接近 0。
        renderer.set_post_process(Some(
            PostProcess::new(1.0, [1.0, 0.0, 0.0, 1.0], 0.0).expect("tint"),
        ));
        let tinted = render_and_read(&mut renderer);
        let centre_tinted = pixel(&tinted, size / 2, size / 2);
        assert!(
            centre_tinted[1] <= 1 && centre_tinted[2] <= 1,
            "tint=[1,0,0] 必须把绿蓝压掉：{centre_tinted:?}"
        );
        assert!(centre_tinted[0] > 0, "红色分量必须保留：{centre_tinted:?}");

        // 暗角 1.0：四角全黑、中心不受影响。
        renderer.set_post_process(Some(
            PostProcess::new(1.0, [1.0; 4], 1.0).expect("vignette"),
        ));
        let vignetted = render_and_read(&mut renderer);
        let corner = pixel(&vignetted, 0, 0);
        let centre = pixel(&vignetted, size / 2, size / 2);
        assert_eq!(corner, [0, 0, 0, 255], "暗角 1.0 时角落必须是黑的");
        assert!(
            centre[0] > corner[0],
            "暗角只压边缘：centre={centre:?} corner={corner:?}"
        );
    }
}
