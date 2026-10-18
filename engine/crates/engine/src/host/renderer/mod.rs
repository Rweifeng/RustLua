//! GPU renderer: device/surface/pipelines, texture upload, depth/shadow
//! resources, 2D/3D render passes, and M8-08 culling/instancing. The scene
//! layer produces [`SceneMesh`]/[`Scene3d`] frames; this module consumes them.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    path::PathBuf,
};

use rustlua_assets::asset_handle::AssetHandle;
use rustlua_camera::camera::{Camera3D, FrameViews, Viewport};
use rustlua_math::math3d::{Mat4, Vec3};
use rustlua_render::async_loader::AsyncLoader;
use rustlua_render::layer::RenderLayer;
use rustlua_render::light::Light;
use rustlua_render::post_process::PostProcess;
use rustlua_render::render_plan::{RenderPass, RenderPlan};
use wgpu::{Device, Queue, Surface, SurfaceConfiguration};

use crate::host::frame::{Scene3d, SceneMesh};

mod arena;
mod construct;
mod gpu_mesh;
mod gpu_texture;
mod passes;
mod readback;
mod targets;
mod timestamp;
mod world_sprite;
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
    /// RGBA8 像素字节数（显存占用估算；性能面板的「贴图显存」由它汇总）。
    bytes: u64,
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
    /// 顶点数（几何账本：`RenderFrameMetrics::vertices` 与缓存字节都靠它）。
    vertex_count: u32,
    /// 顶点 + 索引缓冲字节数（网格缓存显存估算）。
    bytes: u64,
}

/// A decoded base-color texture uploaded to the GPU (sRGB) plus its view.
struct CachedAlbedoTexture {
    /// Kept alive so `view` stays valid; never read directly.
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// RGBA8 像素字节数（显存占用估算）。
    bytes: u64,
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
    /// docs/54 §4.3：GPU 材质由统一的 [`rustlua_render::renderable::Material`] 构造——金属度/粗糙度/
    /// 基色的默认值从此只有一处，精灵与网格不会各自演化出一套"缺省材质"。
    fn from_material(
        material: &rustlua_render::renderable::Material,
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
    material: rustlua_render::renderable::Material,
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
    /// 本帧实际提交的顶点数。2D overlay / 世界 Sprite / 矢量文字按顶点计；
    /// 3D 按「每个实例的顶点数 × 实例数」统计一次（与 `triangles` 同口径，不因阴影 pass 重复）。
    pub vertices: u64,
    /// 本帧的 shader pipeline 状态设置次数（Unity SetPass Calls 的对应量）。
    ///
    /// 每次 `set_pipeline`（含 2D 批次在普通管线与 SDF 管线之间的切换）计一次；
    /// 同一个管线的重复绑定不计。清屏不是绘制通道，不计数。
    pub set_pass_calls: u64,
    /// 本帧**被丢弃**的 draw 数（bind group 造不出来时的兜底分支）。
    ///
    /// 为什么要单独计数：通道级早就有对账（`RenderPlan::mismatch` → 告警 +
    /// `last_frame_passes` 可断言），**draw 级没有**——某个 draw 因为贴图/字体图集上传失败而
    /// 画不出来时，代码只是 `continue`：不计数、不告警，症状是"屏幕上少一块，日志里一个字都没有"。
    /// 这是本仓最不能接受的失败类型；readback 侧早有同款计数（`ReadbackMetrics::dropped`），
    /// 这里补齐同一把尺：**`draw_calls` 偏少 + `dropped_draws > 0` 才是"这一帧少了东西"的完整证据**。
    pub dropped_draws: u64,
}

fn record_draw_call(metrics: &mut RenderFrameMetrics) {
    metrics.draw_calls += 1;
}

/// 记一次 shader 管线状态设置（Unity SetPass Calls 的对应量）。
fn record_set_pass(metrics: &mut RenderFrameMetrics) {
    metrics.set_pass_calls += 1;
}

/// 记一次**画不出来**的 draw。刻意**不加** `draw_calls`：丢弃不是绘制，
/// 两者分账才能从读数上看出"计划里有、实际没画"。
fn record_dropped_draw(metrics: &mut RenderFrameMetrics) {
    metrics.dropped_draws += 1;
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
    metrics.vertices += u64::from(vertex_count);
    metrics.triangles += u64::from(vertex_count) / 3;
}

fn record_vertex_draw(metrics: &mut RenderFrameMetrics, vertex_count: u32) {
    record_draw_call(metrics);
    metrics.vertices += u64::from(vertex_count);
    metrics.instances += u64::from(vertex_count) / 6;
    metrics.triangles += u64::from(vertex_count) / 3;
}

fn record_indexed_draw(
    metrics: &mut RenderFrameMetrics,
    index_count: u32,
    vertex_count: u32,
    instance_count: usize,
    pass_count: u64,
) {
    metrics.draw_calls += pass_count;
    metrics.instances += instance_count as u64;
    metrics.vertices += u64::from(vertex_count) * instance_count as u64;
    metrics.triangles += u64::from(index_count) / 3 * instance_count as u64;
}

/// 渲染器资源缓存与加载队列的一帧快照（编辑器性能面板 / 诊断用）。
///
/// 这是**只读汇总**，不改变任何缓存状态；字节数是按上传时记录的 RGBA8 像素 / 顶点索引
/// 缓冲大小估算的显存占用，不是驱动真实分配量（驱动可能对齐或压缩）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RendererResourceStats {
    /// 已上传的 2D 贴图 + 3D albedo 贴图条数。
    pub loaded_textures: usize,
    /// 2D 贴图 + 3D albedo + 字体图集的 RGBA8 字节数估算。
    pub texture_bytes: u64,
    /// 字体图集纹理条数（内建默认图集 + 自定义字体）。
    pub font_atlas_textures: usize,
    /// 2D 贴图身份缓存命中次数（句柄命中）。
    pub texture_identity_hits: u64,
    /// 2D 贴图身份缓存未命中次数（首次见到的句柄）。
    pub texture_identity_misses: u64,
    /// 已缓存的 3D 网格条数。
    pub cached_meshes: usize,
    /// 3D 网格缓存顶点 + 索引缓冲字节数。
    pub mesh_bytes: u64,
    /// 网格身份缓存命中次数。
    pub mesh_identity_hits: u64,
    /// 网格身份缓存未命中次数。
    pub mesh_identity_misses: u64,
    /// 异步加载队列中尚未取走的请求数。
    pub async_pending: usize,
    /// 已取走的请求数（含失败）。
    pub async_completed: usize,
    /// 后台解码线程数（0 = 同步完成，编辑器离屏预览即如此）。
    pub async_workers: usize,
    /// 加载器当前是否走异步（后台线程）模式。
    pub async_mode: bool,
    /// 2D 顶点缓冲当前容量（字节）。
    pub vertex_buffer_capacity: u64,
    /// 2D 顶点缓冲扩容次数。
    pub vertex_buffer_growths: u64,
    /// 单帧 2D 顶点字节数峰值。
    pub vertex_bytes_peak: u64,
    /// 非阻塞回读账本（槽数 / 丢弃 / CPU 缓冲字节等）。
    pub readback: ReadbackMetrics,
}

/// 把声明的 [`Viewport`]（浮点、世界无关）换算成目标附件内的**等比居中矩形**。
///
/// 换算本身在 [`crate::host::fit::contain_rect`]（与 `game_web::fit` / Canvas2D 层同一口径），
/// 这里只是渲染器侧的调用点。曾经的实现是 `(0, 0, min(declared, target))`——那条路只在
/// "声明盒与附件同比例"时才对，否则内容会挤成左上角一块、其余是底色（微信接管形态实测
/// 内容块 `1380×720` / 表面 `2532×1170`，非底色像素只占 31.1%）。
fn fit_viewport_rect(viewport: Viewport, target: (u32, u32)) -> (u32, u32, u32, u32) {
    crate::host::fit::contain_rect((viewport.width, viewport.height), target)
}

/// 在 pass 上应用声明的视口。
///
/// 换算结果等于整个目标时**不调用** `set_viewport`：wgpu 的默认视口就是整目标，
/// 因此"没声明"、"声明了 ≡ 目标"与"声明了但只差 1 px 取整"三条路径产生完全相同的命令流
/// ——收敛不引入像素差异（桌面/编辑器/浏览器 fit 后的画布都在这三条里）。
fn apply_declared_viewport(
    pass: &mut wgpu::RenderPass<'_>,
    viewport: Option<Viewport>,
    target: (u32, u32),
) {
    let Some(viewport) = viewport else {
        return;
    };
    let (x, y, width, height) = fit_viewport_rect(viewport, target);
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
    /// 设备**有没有存储缓冲**（WebGL2 上是 0）。构造期由 `device.limits()` 定下，之后不变。
    ///
    /// 为什么要有这个字段：`construct.rs` 里三处存储缓冲（矢量文字的两个绑定、3D 材质的实例
    /// 矩阵、实例环）在 `max_storage_buffers_per_shader_stage = 0` 的设备上**建不出来**，
    /// 而校验错误在 wasm 上会把进程 abort。所以按能力决定"建什么"，并把结论留在这里给渲染侧用：
    /// `passes.rs` 据此**不声明**矢量文字与 3D 两个通道（计划里就没有，就不会绑到占位管线），
    /// 同时把丢弃的 draw 记进 `dropped_draws`——少画东西而日志里一个字都没有，是本仓最不接受的失败。
    storage_capable: bool,
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

    /// WP5：声明本帧的相机侧视图（清屏策略 + 视口）。
    ///
    /// 收敛前渲染器把清屏色**写死在 pass 里**、视口恒为整个目标，宿主想说"这块区域
    /// 只画 2D overlay"或"这帧不要清屏"都无处可说。现在这两件事都由 [`rustlua_camera::camera::FrameViews`]
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

    /// 资源缓存 / 加载队列的汇总快照（编辑器性能面板与诊断用）。
    ///
    /// 贴图字节按上传时记录的 RGBA8 面积累加；字体图集按 `wgpu::Texture::size` 现算
    /// （数量很少，且它们只在版本变化时重建）。网格字节在 `upload_geometry` 时记录，
    /// 因此这里只是求和，不会触发任何 GPU 查询。
    #[must_use]
    pub fn resource_stats(&self) -> RendererResourceStats {
        let loaded_texture_bytes: u64 = self.textures.iter().map(|texture| texture.bytes).sum();
        let albedo_bytes: u64 = self
            .albedo_textures
            .iter()
            .map(|texture| texture.bytes)
            .sum();
        let font_bytes = self
            .font_atlas_texture
            .as_ref()
            .map(texture_rgba8_bytes)
            .unwrap_or(0)
            + self
                .custom_fonts
                .values()
                .filter_map(|font| font.texture.as_ref())
                .map(texture_rgba8_bytes)
                .sum::<u64>();
        let font_atlas_textures = usize::from(self.font_atlas_texture.is_some())
            + self
                .custom_fonts
                .values()
                .filter(|font| font.texture.is_some())
                .count();
        let mesh_bytes: u64 = self
            .mesh_cache
            .values()
            .flat_map(|mesh| mesh.primitives.iter())
            .map(|primitive| primitive.geometry.bytes)
            .sum();
        RendererResourceStats {
            loaded_textures: self.textures.len() + self.albedo_textures.len(),
            texture_bytes: loaded_texture_bytes + albedo_bytes + font_bytes,
            font_atlas_textures,
            texture_identity_hits: self.texture_identity_hits,
            texture_identity_misses: self.texture_identity_misses,
            cached_meshes: self.mesh_cache.len(),
            mesh_bytes,
            mesh_identity_hits: self.mesh_identity_hits,
            mesh_identity_misses: self.mesh_identity_misses,
            async_pending: self.async_loader.pending_count(),
            async_completed: self.async_loader.completed_count(),
            async_workers: self.async_loader.workers(),
            async_mode: self.async_loader.mode() == rustlua_render::async_loader::LoadMode::Async,
            vertex_buffer_capacity: self.vertex_buffer.size(),
            vertex_buffer_growths: self.vertex_buffer_growths,
            vertex_bytes_peak: self.vertex_bytes_peak,
            readback: self.readback_metrics(),
        }
    }
}

/// RGBA8 纹理的字节数估算（字体图集与 albedo 都是 Rgba8Unorm/Rgba8UnormSrgb）。
fn texture_rgba8_bytes(texture: &wgpu::Texture) -> u64 {
    let size = texture.size();
    u64::from(size.width) * u64::from(size.height) * u64::from(size.depth_or_array_layers) * 4
}

/// Conservative local-space bounding sphere from a primitive's vertex AABB.
fn compute_mesh_bounds(vertices: &[rustlua_render::mesh3d::MeshVertex]) -> (Vec3, f32) {
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
    let mut data = Vec::with_capacity(rustlua_render::mesh3d::MAX_BONES * 4);
    for matrix in palette.iter().take(rustlua_render::mesh3d::MAX_BONES) {
        data.extend_from_slice(&matrix.columns);
    }
    while data.len() < rustlua_render::mesh3d::MAX_BONES * 4 {
        data.extend_from_slice(&IDENTITY_MAT4);
    }
    data
}

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
/// `Light::direction` is the direction the light **travels** (`rustlua_render::light` 与 docs/24 的约定，
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
