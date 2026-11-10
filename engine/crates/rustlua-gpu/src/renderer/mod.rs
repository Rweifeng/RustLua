//! GPU renderer: device/surface/pipelines, texture upload, depth/shadow
//! resources, 2D/3D render passes, and M8-08 culling/instancing. The scene
//! layer produces [`SceneMesh`]/[`Scene3d`] frames; this module consumes them.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::Arc,
};

use rustlua_assets::asset_handle::AssetHandle;
use rustlua_assets::env_asset::EnvironmentAsset;
use rustlua_camera::camera::{Camera3D, FrameViews, Viewport};
use rustlua_math::math3d::{Aabb3, Mat4, Vec3};
use rustlua_render::async_loader::AsyncLoader;
use rustlua_render::layer::RenderLayer;
use rustlua_render::light::{Light, MAX_POINT_LIGHTS, PointLight};
use rustlua_render::post_process::{PostProcess, Tonemap};
use rustlua_render::render_plan::{RenderPass, RenderPlan};
use rustlua_render::render_settings::{
    EnvironmentLighting, EnvironmentSettings, IblMode, IblTier, RenderSettings, SceneColorFormat,
    ibl_tier, resolve_ibl,
};
use wgpu::{Device, Queue, Surface, SurfaceConfiguration};

use rustlua_render::frame::{Scene3d, SceneMesh};
use rustlua_render::instance_batch::InstanceBatch3D;

pub use environment::EnvironmentMapError;

mod arena;
mod batch_motion_gpu;
mod capabilities;
mod compute;
mod construct;
mod environment;
mod gpu_mesh;
mod gpu_particles;
mod gpu_texture;
mod pass_timestamps;
mod passes;
pub mod pbr;
pub mod pipeline_cache;
mod readback;
#[cfg(not(target_arch = "wasm32"))]
mod shader_hot_reload;
// R05-4：编辑器与桌面运行时要把重载读数显示给作者，所以这三个类型是**对外**的；
// wasm 侧没有覆盖目录，也就没有报告类型可导出。
#[cfg(not(target_arch = "wasm32"))]
pub use shader_hot_reload::{ShaderReloadError, ShaderReloadReport, ShaderSourceStatus};
mod targets;
mod timestamp;
mod world_sprite;
pub use batch_motion_gpu::BatchMotionGpuError;
pub use capabilities::RendererCapabilities;
pub use compute::{ComputeBinding, ComputeDispatch, ComputeError, ComputeRequest, ComputeStats};
pub use gpu_particles::{
    GpuParticleConfig, GpuParticleConfig3D, GpuParticleSnapshot, GpuParticleState,
    GpuParticleStatus, GpuParticleStep, GpuParticleStreamConfig, MAX_GPU_PARTICLE_SLOTS,
    MAX_GPU_PARTICLE_SYSTEMS,
};
pub use pbr::{DirectBrdf, direct_brdf, normal_matrix};
use pipeline_cache::PipelineCache;
pub use pipeline_cache::PipelineCacheStats;
pub use readback::{
    ReadbackMetrics, ReadbackStatus, SceneColorReadback, write_capture_placeholder,
};
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
///
/// `view` + `sampling` 是 M79 P1-3c 加的：换导入设置（滤镜/寻址）要**就地重建绑定组**，
/// 而重建只需要视图和采样器——没有视图就得重传像素，没有记录当前采样就不知道该不该重建。
struct LoadedTexture {
    bind_group: wgpu::BindGroup,
    /// RGBA8 像素字节数（显存占用估算；性能面板的「贴图显存」由它汇总）。
    bytes: u64,
    view: wgpu::TextureView,
    sampling: rustlua_assets::import_settings::TextureImportSettings,
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

/// A decoded material texture uploaded to the GPU (sRGB color or linear data) plus its view.
struct CachedMaterialTexture {
    /// Kept alive so `view` stays valid; never read directly.
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// RGBA8 像素字节数（显存占用估算）。
    bytes: u64,
}

/// Resolved GPU texture slots for one material. Indices point into
/// `Renderer::albedo_textures` (sRGB) or `Renderer::linear_textures` (data).
#[derive(Clone, Copy, Default)]
struct MaterialTextureIndices {
    base_color: Option<u32>,
    metallic_roughness: Option<u32>,
    emissive: Option<u32>,
    normal: Option<u32>,
    occlusion: Option<u32>,
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
    /// Index into `Renderer::linear_textures`; `None` samples white (factor passthrough).
    metallic_roughness: Option<u32>,
    /// `emissive.rgb * emissive_strength` (linear).
    emissive: [f32; 3],
    /// Index into `Renderer::albedo_textures`; `None` samples white.
    emissive_texture: Option<u32>,
    /// Index into `Renderer::linear_textures`; `None` samples flat +Y (no perturbation).
    normal_texture: Option<u32>,
    normal_scale: f32,
    /// Index into `Renderer::linear_textures`; `None` samples white (no occlusion).
    occlusion_texture: Option<u32>,
    occlusion_strength: f32,
    /// P3-R02-4：alpha 模式与阈值决定主通道的 discard / 混合管线。
    alpha_mode: rustlua_render::mesh3d::MaterialAlphaMode,
    alpha_cutoff: f32,
    /// 双面材质选独立剔除状态的管线（背面不再被光栅化掉）。
    double_sided: bool,
    /// Built-in terrain detail-atlas tile size in world units; `0` = flat
    /// procedural ground. Reaches the shader as `Material.surface.w`.
    detail_scale: f32,
    /// P3-R162：清漆层（第二 GGX 瓣，F0=0.04）；0 = 关闭。
    clearcoat: f32,
    clearcoat_roughness: f32,
    /// P3-R163：SSS 近似（wrap 漫反射 + 背向透光，albedo 染色）；0 = 关闭。
    sss_strength: f32,
    sss_thickness: f32,
    /// P3-R164：薄膜干涉虹彩（复眼/甲壳）；0 = 关闭。
    iridescence: f32,
    iridescence_ior: f32,
    iridescence_thickness_min: f32,
    iridescence_thickness_max: f32,
    /// P3-R165：绒毛/布面光泽瓣（ASA 分布，逆光绒边）；颜色全 0 = 关闭。
    sheen_color: [f32; 3],
    sheen_roughness: f32,
    /// P3-R197：水面廉价光照档（`__water__?...&cheap=1`）。只对水面有意义，缺省 false。
    water_cheap: bool,
    /// P3-R203：地形廉价细节档（`__terrain__?...&cheap=1`）。只对地形有意义，缺省 false。
    terrain_cheap: bool,
}

/// P3-R166：`sect=1` 地形的高度图，供 Tonemap 在裁剪平面处封出实体土壤剖面。
/// 高度以 16 位定点打包在 R/G 双通道里，采样用 `textureLoad`（不插值）。
#[derive(Clone)]
struct SectionHeightMap {
    view: wgpu::TextureView,
    /// `height_range()` 的下界与跨度，着色器用它把定点值还原成世界 y。
    min_y: f32,
    span: f32,
    /// 地形半边长度（世界单位）；着色器用它把局部 xz 映射到高度图 UV。
    half: f32,
    /// 分块中心（世界 xz）。高度图烘的是整张地形，着色器把世界坐标折回
    /// 整张地形局部坐标时要补上这个偏移；整张地形为 `[0, 0]`。
    origin: [f32; 2],
}

/// Cached GPU geometry + file materials for one mesh path.
struct CachedMesh {
    primitives: Vec<CachedPrimitive>,
    /// 只有 `sect=1` 的整张地形才有；其它网格保持 `None`，Tonemap 不封口。
    section_height: Option<SectionHeightMap>,
}

impl GpuMaterial {
    /// P3-R02：GPU 材质由统一的 [`rustlua_render::mesh3d::MaterialAsset`] 构造。glTF 与
    /// `.material` 都先落到这个资产类型，GPU 侧因此只有一条材质解释路径。
    fn from_material(
        material: &rustlua_render::mesh3d::MaterialAsset,
        textures: MaterialTextureIndices,
        skinned: bool,
    ) -> Self {
        Self {
            base_color: material.base_color,
            metallic: material.metallic,
            roughness: material.roughness,
            skinned,
            albedo: textures.base_color,
            metallic_roughness: textures.metallic_roughness,
            emissive: [
                material.emissive[0] * material.emissive_strength,
                material.emissive[1] * material.emissive_strength,
                material.emissive[2] * material.emissive_strength,
            ],
            emissive_texture: textures.emissive,
            normal_texture: textures.normal,
            normal_scale: material.normal_scale,
            occlusion_texture: textures.occlusion,
            occlusion_strength: material.occlusion_strength,
            alpha_mode: material.alpha_mode,
            alpha_cutoff: material.alpha_cutoff,
            double_sided: material.double_sided,
            detail_scale: material.detail_scale,
            clearcoat: material.clearcoat,
            clearcoat_roughness: material.clearcoat_roughness,
            sss_strength: material.sss_strength,
            sss_thickness: material.sss_thickness,
            iridescence: material.iridescence,
            iridescence_ior: material.iridescence_ior,
            iridescence_thickness_min: material.iridescence_thickness_min,
            iridescence_thickness_max: material.iridescence_thickness_max,
            sheen_color: material.sheen_color,
            sheen_roughness: material.sheen_roughness,
            water_cheap: false,
            terrain_cheap: false,
        }
    }
}

struct CachedPrimitive {
    geometry: GpuGeometry,
    /// GPU 侧已解析的材质（贴图槽位 + 参数）。展平成 `GpuMaterial` 是为了不在每帧
    /// 克隆 glTF 内嵌像素；`skinned` 由绘制侧的动画调色板决定，这里恒为 false。
    material: GpuMaterial,
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
    /// P3-R170：`__water__` 哨兵网格。平面反射 pass 跳过它（水面是镜面本身）。
    water: bool,
    /// P3-R188：组内全部实例的世界包围盒并集。点光阴影 pass 用它做 range 剔除
    /// （range 外的投射者不可能遮挡 range 内的接收者 ⇒ 数学精确，不改像素）。
    bounds: Aabb3,
    /// P3-R245：`Some` = 这份 draw 的实例矩阵由 compute 现场求值（`models` 保留作回退：
    /// 烘焙失败/无 compute 能力时仍按 CPU 路径画）。
    ///
    /// P3-R246：`models` 可以为空（宿主不再每帧求 CPU 矩阵）——此时实例数看批次本身，
    /// 回退矩阵在烘焙失败时就地按 `matrices_at` 补算，不回退到"空模型表的 draw"。
    gpu_motion: Option<FrameGpuMotion>,
    /// P3-R255：这批实例的网格局部包围球半径（从局部原点算，含球心偏移），供视锥剔除
    /// 在 GPU 侧展开成逐实例保守球（再乘 max|scale|·model_scale）。非 GPU 批恒 0。
    gpu_motion_local_radius: f32,
    /// P3-R248：这条 draw 是否进方向光 CSM 链。`false` = 被分类排除档按样本名/网格路径
    /// 掐掉（静态合批组只按路径判），主视图/平面反射照旧；成组/建 draw 时一次算好。
    cast_shadow: bool,
    /// P3-R265：这条 draw 是否进点光 6 面链。默认与 `cast_shadow` 同值（`shadow_caster_skip`
    /// 两链一起掐）；点光专用排除档只清这一位 ⇒ CSM 的 crowd 阴影保留、6 面链的 crowd
    /// 提交掐掉，主视图/平面反射照旧。
    cast_point_shadow: bool,
}

impl FrameDraw {
    /// 这条 draw 实际要画的实例数。
    ///
    /// P3-R246：GPU 位姿批次的 `models` 可能是空的（宿主跳过了 CPU 求值），实例数以批次为准；
    /// 其余情况就是 `models.len()`。画调用、记账、合批日志都必须用这一个口径，否则
    /// "画 N 个却只上传 0 个"会读到邻居的矩阵。
    fn instance_count(&self) -> usize {
        self.gpu_motion
            .as_ref()
            .map_or(self.models.len(), |motion| motion.batch.instances.len())
    }
}

/// P3-R245：一份 draw 的 GPU 位姿求值输入。批次按 `Arc` 共享（同一批次的多个
/// primitive 各一条 draw，批次数据只存一份）。
struct FrameGpuMotion {
    batch: Arc<InstanceBatch3D>,
    /// 求值时刻（场景时钟秒）。
    time: f32,
    /// 导入设置的模型缩放（CPU `scaled_model` 同式）。
    model_scale: f32,
}

/// Static drawables sharing one mesh path + primitive, accumulated for a
/// single instanced draw call.
struct StaticGroup {
    geometry: GpuGeometry,
    material: GpuMaterial,
    models: Vec<Mat4>,
    water: bool,
    /// P3-R188：全部实例世界包围盒的并集（组内逐实例扩展）。
    bounds: Aabb3,
    /// P3-R248：整组是否进方向光 CSM 链（组键里的网格路径命中排除子串 ⇒ false）。
    cast_shadow: bool,
    /// P3-R265：整组是否进点光 6 面链（组键路径命中点光专用排除子串 ⇒ false；
    /// `cast_shadow` 为假时恒假）。
    cast_point_shadow: bool,
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
    /// 所有通道的 instance 总数（含 2D/粒子/3D）。
    pub instances: u64,
    /// 仅 3D indexed mesh 的 draw 数（按 shadow pass 数计），不含 2D/粒子。
    pub mesh3d_draw_calls: u64,
    /// 仅 3D indexed mesh 的实例数，不含 2D/粒子。
    pub mesh3d_instances: u64,
    /// P3-R183：本帧参与 3D 绘制的 drawable×primitive 候选数（网格已缓存、进入剔除判定）。
    ///
    /// 与 `cull_frustum` 一起构成「剔掉多少实例」的门读数：候选 − 剔除 = 进入合批/提交的
    /// 实例数。剔除关闭时 `cull_frustum` 恒 0，候选数不变——两档 A/B 因此可比。
    pub cull_candidates: u64,
    /// P3-R183：本帧被剔除的 drawable×primitive 数（`cull_enabled` 关闭时视锥部分恒 0）。
    /// P3-R206 起含**最大绘制距离**剔除（该档默认关闭，计数与视锥共用同一分母）。
    pub cull_frustum: u64,
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
    /// P3-R01：本帧场景颜色是否真的走了 HDR（`Rgba16Float` + Tonemap）。
    ///
    /// `false` 不代表出错：策略关闭、设备不支持、纯 2D 帧、曝光归属冲突都会得到 `false`；
    /// 原因由启动日志与每次冲突的一条告警给出。
    pub hdr_active: bool,
    /// 本帧 Tonemap 曲线；没有 Tonemap 通道（LDR）时为 `None`。
    pub tonemap: Option<Tonemap>,
    /// 本帧场景颜色目标的格式类别（HDR 为 `Rgba16Float`，LDR 按 sRGB/线性归类）。
    pub scene_format: SceneColorFormat,
    /// 本帧 Tonemap 是否实际执行了泛光（HDR 且 bloom > 0）。
    pub bloom_active: bool,
    /// 本帧 Tonemap 是否实际执行了 FXAA-lite（HDR 且 `RenderSettings.fxaa`）。
    pub fxaa_active: bool,
    /// 本帧方向光是否走了 3×3 PCF 软阴影分支。
    pub soft_shadows_active: bool,
    /// 本帧 Tonemap 是否实际执行了 SSAO-lite（HDR 且 `RenderSettings.ssao`）。
    pub ssao_active: bool,
    /// 本帧 Tonemap 是否实际执行了 SSR-lite（HDR 且 `RenderSettings.ssr`）。
    pub ssr_active: bool,
    /// P3-R170：本帧是否真的跑了水面平面反射（PBR + HDR + SSR + 场景有水面 + 有 3D 网格）。
    pub planar_reflection_active: bool,
    /// 平面反射 pass 里的 draw 数（0 与 `planar_reflection_active` 同时成立 = 空 pass）。
    pub planar_reflection_draws: u64,
    /// 本帧 Tonemap 是否实际执行了屏幕空间接触阴影。
    pub contact_shadows_active: bool,
    /// 本帧方向光是否走了 ESM filtered shadow。
    pub filtered_shadows_active: bool,
    /// 本帧 Tonemap 是否实际执行了距离雾。
    pub fog_active: bool,
    /// 本帧 Tonemap 是否实际画了程序化天空（HDR + 雾 + 有场景相机）。
    pub sky_active: bool,
    /// 本帧 Tonemap 是否实际执行了屏幕空间神光。
    pub god_rays_active: bool,
    /// 本帧 Tonemap 是否实际执行了屏幕空间景深。
    pub dof_active: bool,
    /// 本帧是否渲染了前八盏（最多 8 盏）点光的立方体阴影。
    pub point_shadows_active: bool,
    /// P3-R180：`render()` 里 surface acquire 的 CPU 时间；离屏路径恒 0。
    pub cpu_acquire_us: u64,
    /// P3-R180：`output.present()` 的 CPU 时间；离屏路径恒 0。
    ///
    /// 为什么单列：宿主 `host_phases.submit` 包住的是 acquire + 录制 + `queue.submit` +
    /// present 四件事（`game_app.rs`）。有帧步调/垂直同步时 present 会把整段吃掉，
    /// 那时把录制优化到极致也不动读数——不拆开就只能靠猜。
    pub cpu_present_us: u64,
    /// P3-R180：命令录制（`render_passes` 全程，含 uniform 写入与实例合批）的 CPU 时间。
    pub cpu_record_us: u64,
    /// P3-R180：`queue.submit` 本身的 CPU 时间（不含录制、不含 present）。
    pub cpu_submit_us: u64,
    /// P3-R180：录制里方向光阴影链（4 级联 + ESM 全屏）的 CPU 时间。
    pub record_shadow_us: u64,
    /// P3-R180：录制里点光立方体阴影（最多 8 盏 × 6 面，每面重画全部 drawable）的 CPU 时间。
    pub record_point_shadow_us: u64,
    /// P3-R180：本帧录制的方向光阴影 pass 数（4 级联，开 `filtered_shadows` 时再 +4 个 ESM 全屏）。
    ///
    /// 为什么要计数而不是只计时：跳过空 pass 的优化必须能被断言，而 CPU 计时在本机是
    /// 毫秒级噪声；pass 数是确定量。
    pub shadow_passes_recorded: u64,
    /// P3-R180：本帧录制的点光阴影像 pass 数（盏数 × 6，`point_shadows` 关或无点光时为 0）。
    pub point_shadow_passes_recorded: u64,
    /// P3-R188：点光阴影 pass 里因落在光照 range 外被剔除的 draw 次数（跨全部面累计；
    /// 0 = 剔除没起作用或关闭）。与 `point_shadow_passes_recorded` 同口径可断言。
    pub point_shadow_draws_culled: u64,
    /// P3-R249：点光链里被分类排除档（`shadow_caster_skip`，或 P3-R265 只掐点光链的
    /// `point_shadow_caster_skip`）挡在 6 面之外的 draw 次数
    /// （跨灯 × 面累计，按"本来会提交"计）。与方向光的 `shadow_draws_skipped` 分开记账：
    /// 两条链的尝试分母不同，混在一起就没有守恒式可对账。
    pub point_shadow_draws_skipped: u64,
    /// P3-R194：点光阴影 pass 里因落在该面相机视锥外被剔除的 draw 次数（跨灯×面累计；
    /// range 剔除之后的第二层，面视锥就是 GPU 的裁剪体 ⇒ 只省不丢）。
    pub point_shadow_face_draws_culled: u64,
    /// P3-R189：方向光阴影 pass 里因落在该级联裁剪体外的 draw 次数（跨 4 级联累计）。
    /// 级联盒就是阴影相机的裁剪体，盒外几何 GPU 本来也会裁掉 ⇒ 只省不丢。
    pub shadow_cascade_draws_culled: u64,
    /// P3-R247：方向光阴影链里**实际提交**的 draw 次数（跨级联累计，不含 ESM 全屏 quad）。
    ///
    /// 与 `shadow_cascade_draws_culled` 同帧对账：`shadow_draws_drawn + culled`
    /// 恒等于「drawable 数 × 实际录制的级联数」。为什么要读实际提交：批次被拆成多条
    /// draw 后，逐级联剔除的粒度变了，"阴影便宜了"必须能拆成"少录了几次 draw"。
    pub shadow_draws_drawn: u64,
    /// P3-R247：同口径累计的实例数（每条提交的 `instance_count()` 之和，跨级联）。
    ///
    /// 阴影 pass 是几何/顶点瓶颈时，这个数就是下一刀（逐实例剔除/LOD）的分母：
    /// 它大于 0 说明确实有实例级余量，接近 0 说明只剩静态物。
    pub shadow_instances_drawn: u64,
    /// P3-R261：点光 6 面链里**实际提交**的 draw 次数（跨灯 × 面累计）。
    ///
    /// 为什么单列：`shadow_draws_drawn` / `shadow_instances_drawn` 只统计方向光 CSM 链。
    /// P3-R260 实测「150 m 预算把 CSM 实例砍 97.7 %、shadow pass 只掉 4.5 %」，正是因为
    /// 点光链的 crowd 提交一直在跑，却没有任何一列在数它（§21.540）。
    pub point_shadow_draws_drawn: u64,
    /// P3-R261：同口径的点光链提交实例数（跨灯 × 面累计）。
    pub point_shadow_instances_drawn: u64,
    /// P3-R248：被分类排除档（`shadow_caster_skip`）挡在 CSM 链外的 draw 次数
    /// （跨级联累计，按"本应提交"计）。与 `shadow_draws_drawn` 分开记：一个是视锥剔除
    /// （只省不丢），一个是用户显式要求的观感取舍（会丢阴影）。
    pub shadow_draws_skipped: u64,
    /// P3-R256：被距离预算挡在 CSM 链外的 draw 次数（跨级联累计，按"本应提交"计）。
    /// 与分类排除分开：两档可叠加，守恒式仍是
    /// `提交 + 级联剔除 + 分类排除 + 距离预算 = drawable × 级联`。
    pub shadow_distance_skipped: u64,
    /// P3-R256：点光链里被距离预算挡在 6 面之外的 draw 次数（跨灯 × 面累计）。
    pub point_shadow_distance_skipped: u64,
    /// P3-R191：平面反射 pass 里因落在镜像相机视锥外的 draw 次数（水面与 Blend 不计）。
    pub planar_draws_culled: u64,
}

fn record_draw_call(metrics: &mut RenderFrameMetrics) {
    metrics.draw_calls += 1;
}

/// E06-3：一条渲染通道在 GPU 上的时长。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassGpuTiming {
    pub pass: RenderPass,
    pub gpu_us: u64,
}

/// E06-3：上一帧的逐通道 GPU 时间报告。
///
/// `supported=false` 是设备没有 timestamp 查询能力（或回读失败），此时 `passes` 为空、
/// `frame_us` 为 `None`——**不用 0 冒充**。`consistent` 只在同一次提交同时拿到整帧与
/// 至少一条通道时间时才有值；判据是 `|pass_sum - frame| / frame ≤ 5%`。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuPassReport {
    pub supported: bool,
    pub reason: Option<String>,
    pub frame_us: Option<u64>,
    pub passes: Vec<PassGpuTiming>,
    pub pass_sum_us: u64,
    pub delta_pct: Option<f32>,
    pub consistent: Option<bool>,
}

impl GpuPassReport {
    #[must_use]
    pub fn pass_us(&self, pass: RenderPass) -> Option<u64> {
        self.passes
            .iter()
            .find(|timing| timing.pass == pass)
            .map(|timing| timing.gpu_us)
    }

    /// 面板「复制报告」与无头证据共用的一行式文本。
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = match self.frame_us {
            Some(frame_us) => {
                let mut line = format!("gpu passes: frame={frame_us}us sum={}us", self.pass_sum_us);
                if let Some(delta) = self.delta_pct {
                    line.push_str(&format!(" delta={delta:+.2}%"));
                }
                match self.consistent {
                    Some(true) => line.push_str(" consistent"),
                    Some(false) => line.push_str(" INCONSISTENT"),
                    None => {}
                }
                line
            }
            None if self.supported => "gpu passes: no sample yet".to_owned(),
            None => format!(
                "gpu passes: unavailable ({})",
                self.reason.as_deref().unwrap_or("unsupported")
            ),
        };
        for timing in &self.passes {
            text.push_str(&format!(
                "\n  {}: {}us",
                timing.pass.as_str(),
                timing.gpu_us
            ));
        }
        text
    }
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

/// P3-R07-3：instanced quad（GPU 粒子）的一次绘制。
///
/// 不能复用 [`record_vertex_draw`]：那条按"非实例化 6 顶点/精灵"折算 `instances`，
/// 这里 `vertex_count` 恒为 4，除下来会得到 `instances += 0`，读数为"每帧画了几千个粒子、
/// 实例数却是 0"。分账口径反过来也一样：instanced 的顶点数是**每实例**的，必须乘实例数。
fn record_instanced_quad_draw(metrics: &mut RenderFrameMetrics, instance_count: u32) {
    record_draw_call(metrics);
    metrics.instances += u64::from(instance_count);
    metrics.vertices += u64::from(gpu_particles::QUAD_VERTEX_COUNT) * u64::from(instance_count);
    metrics.triangles += 2 * u64::from(instance_count);
}

fn record_indexed_draw(
    metrics: &mut RenderFrameMetrics,
    index_count: u32,
    vertex_count: u32,
    instance_count: usize,
    pass_count: u64,
) {
    metrics.draw_calls += pass_count;
    // Один логический mesh draw может повторяться в depth/shadow passes.
    // Отдельный счётчик считает его один раз, чтобы F3 мог показать геометрию без pass multiplier.
    metrics.mesh3d_draw_calls += 1;
    metrics.instances += instance_count as u64;
    metrics.mesh3d_instances += instance_count as u64;
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
    /// P3-R04-3：已编译的 3D 网格/阴影 shader 变体数。
    pub pipeline_variants: usize,
    /// 变体缓存命中次数（构造期重复请求同一键）。
    pub pipeline_variant_hits: u64,
    /// 变体缓存未命中次数（= 编译尝试次数）。
    pub pipeline_variant_misses: u64,
    /// 启动期变体编译累计耗时（毫秒）。
    pub pipeline_compile_ms: f64,
    /// 非阻塞回读账本（槽数 / 丢弃 / CPU 缓冲字节等）。
    pub readback: ReadbackMetrics,
}

/// 把声明的 [`Viewport`]（浮点、世界无关）换算成目标附件内的**等比居中矩形**。
///
/// 换算本身在 [`rustlua_render::fit::contain_rect`]（与 `game_web::fit` / Canvas2D 层同一口径），
/// 这里只是渲染器侧的调用点。曾经的实现是 `(0, 0, min(declared, target))`——那条路只在
/// "声明盒与附件同比例"时才对，否则内容会挤成左上角一块、其余是底色（微信接管形态实测
/// 内容块 `1380×720` / 表面 `2532×1170`，非底色像素只占 31.1%）。
fn fit_viewport_rect(viewport: Viewport, target: (u32, u32)) -> (u32, u32, u32, u32) {
    rustlua_render::fit::contain_rect((viewport.width, viewport.height), target)
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

/// 把模型矩阵**在自身局部空间**里乘一个统一倍数（导入设置里的"模型缩放"）。
///
/// 只缩前 3 列（基向量），平移列原样保留 —— 这正是 `model × diag(s,s,s,1)` 的结果。
/// 在**画之前**改矩阵（而不是改网格顶点）有三个好处：不重导网格、不动网格缓存，
/// 阴影通道与主通道共用同一份 draw 列表，所以阴影也跟着变。
fn scaled_model(model: Mat4, scale: f32) -> Mat4 {
    if scale == 1.0 {
        return model;
    }
    let mut columns = model.columns;
    for column in columns.iter_mut().take(3) {
        column[0] *= scale;
        column[1] *= scale;
        column[2] *= scale;
    }
    Mat4 { columns }
}
/// 3D 相机 uniform 的 CPU 侧布局（与 `mesh3d_pbr.wgsl` 的 `Camera` 对应）：
/// 16 个 f32 的列主序视图投影 + 4 个 f32 的世界坐标位置（w 现在装渲染时钟秒数）。
///
/// `position.w` 原先固定为 1；P3-R02 起没有着色器读它，因此复用成水面等
/// 程序化材质的动画时钟。旧 `mesh3d.wgsl` 只声明前 64 字节；同一个 buffer
/// 因此仍能同时服务回滚档与 PBR 档。
// 一个 uniform 字段一个参数：打包成结构体后调用点看不出哪一段对应 WGSL 的哪一行。
#[allow(clippy::too_many_arguments)]
pub(super) fn camera_uniform(
    view_projection: &Mat4,
    position: Vec3,
    time: f32,
    clip_plane: [f32; 4],
    cloud_coverage: f32,
    cloud_speed: f32,
    cloud_base: f32,
    cloud_shadow_strength: f32,
    water_tint: [f32; 4],
    surface: [f32; 2],
    ripple: [f32; 2],
    ripple_extra: [f32; 4],
    clip_cap: f32,
) -> [f32; 44] {
    let mut data = [0.0; 44];
    for (index, column) in view_projection.columns.iter().enumerate() {
        data[index * 4..index * 4 + 4].copy_from_slice(column);
    }
    data[16] = position.x;
    data[17] = position.y;
    data[18] = position.z;
    data[19] = time.max(0.0);
    // 世界空间裁剪平面：`n·x + d >= 0` 保留。零向量 = 不裁剪（着色器跳过分支）。
    data[20..24].copy_from_slice(&clip_plane);
    data[24] = cloud_coverage.clamp(0.0, 1.0);
    data[25] = cloud_speed.clamp(0.0, 4.0);
    data[26] = cloud_base.clamp(40.0, 200.0);
    data[27] = cloud_shadow_strength.clamp(0.0, 0.6);
    data[28..32].copy_from_slice(&water_tint);
    data[32] = surface[0].clamp(0.0, 1.0);
    data[33] = surface[1].clamp(0.0, 1.0);
    // P3-R135：水面涟漪点（世界 xz 打包成 u32 位）与年龄（秒）。
    data[34] = ripple[0];
    data[35] = ripple[1];
    data[36..40].copy_from_slice(&ripple_extra);
    // P3-R209：剖面封口深度。0（含 NaN/负值）= 旧的 0.08 薄带，默认像素逐字节不变。
    data[40] = clip_cap.max(0.0);
    data
}

pub struct Renderer {
    /// Present surface in window mode; `None` in offscreen (embedded) mode.
    surface: Option<Surface<'static>>,
    device: Device,
    queue: Queue,
    pub config: SurfaceConfiguration,
    /// M74-03 P0：构造期采集的设备能力快照（只读）。`capabilities().compute` 是所有
    /// compute 消费者的唯一判据，不在别处重复探测。
    capabilities: RendererCapabilities,
    /// M74-03 P1：compute 管线/绑定组缓存、统计与宿主开关。
    compute_state: compute::ComputeState,
    /// M74-03 P2：GPU 粒子（第一个真实消费者）。P3-R07-2 起是**池**：一次 burst 一个系统，
    /// 空 = 未启用，不产生任何额外 pass。
    gpu_particles: Vec<gpu_particles::GpuParticleSystem>,
    /// 全局风加速度（与 CPU 粒子池同口径）；NaN/Infinity 不写入。
    gpu_particle_wind: [f32; 2],
    /// 全局风的 Z 分量；与 `gpu_particle_wind` 分开存，旧的 2D getter 口径不变。
    gpu_particle_wind_z: f32,
    /// 风场：高度剪切 + 空间阵风 `(shear, gust, gust_hz)`；与 CPU 池同一公式。
    gpu_particle_wind_shear: f32,
    gpu_particle_wind_gust: f32,
    gpu_particle_wind_gust_hz: f32,
    /// 最多 4 个局部风源，`[x, y(height), z, radius, radial, lift]`。
    gpu_particle_wind_sources: [[f32; 7]; 4],
    /// 粒子湍流时钟（秒）；与 CPU ParticleSystem.time 同相位来源。
    gpu_particle_time: f32,
    /// P3-R234：网格体积风/温度浮力场的显存副本（`None` = 无场 ⇒ GPU 走原 shader）。
    gpu_particle_grid: Option<gpu_particles::GpuParticleGrid>,
    /// P3-R80：脚本/天气覆盖的云量（0..1）；0.5 是默认覆盖度。
    cloud_coverage: f32,
    /// P3-R82：云层漂移速度倍率（0..4，默认 1）。
    cloud_speed: f32,
    /// P3-R83：云底高度（世界 y，40..200，默认 100）。
    cloud_base: f32,
    /// P3-R84：云影强度（0..0.6，默认 0.26）。
    cloud_shadow_strength: f32,
    /// P3-R86：暴雨闪电的屏幕闪光（0..1）。
    flash_intensity: f32,
    /// P3-R89：水体染色 rgb + 浑浊度 w。
    water_tint: [f32; 4],
    /// P3-R93：地面覆盖 [积雪, 湿润]（0..1）。
    surface_cover: [f32; 2],
    /// P3-R105：脚本曝光偏移（EV，-4..4）。
    exposure_offset: f32,
    /// P3-R135：涟漪点打包位 + 年龄（秒），最多 3 个。
    ripple_packed: [f32; 3],
    ripple_age: [f32; 3],
    ripple_next: usize,
    /// P3-R140：暗角/颗粒（0..1）。
    vignette: f32,
    grain: f32,
    /// 分配给下一个 GPU 粒子系统的绑定组身份（单调递增，回收时按它剔除缓存）。
    gpu_particle_generation: u64,
    /// P3-R07-3：instanced 粒子 draw 段自己的 group2 布局（参数 uniform + 状态只读存储）。
    ///
    /// 构造期只建布局、不建管线（见 [`super::construct`] 的注释）：粒子管线按目标格式延迟
    /// 编译，而布局必须留在渲染器上，否则延迟编译没有第二份"和首帧一致"的描述符可用。
    ///
    /// `None` = 设备没有存储缓冲（WebGL2），布局无法创建；GPU 粒子在这种设备上走 CPU。
    gpu_particle_draw_layout: Option<wgpu::BindGroupLayout>,
    /// P3-R107：group3 深度布局（粒子软边）。
    gpu_particle_depth_layout: Option<wgpu::BindGroupLayout>,
    /// instanced 粒度的管线布局：group0=世界 Sprite 相机、group1=贴图/采样器（两者与
    /// [`Self::world_sprite_camera_bind_group`]、`white` 复用同一份绑定组）、group2=粒子资源。
    /// 与 [`Self::gpu_particle_draw_layout`] 同生共死。
    gpu_particle_pipeline_layout: Option<wgpu::PipelineLayout>,
    /// 已编译的 instanced 粒子管线与其目标格式（按格式最多两条）。空 = 本进程还没画过、
    /// 也没预热过 GPU 粒子。
    ///
    /// HDR 与 LDR 是两条真实变体（附件格式不同），所以按格式各缓存一条；默认仍是第一次
    /// 绘制才编译（启动预算不为"没配 GPU 粒子"的进程付管线的钱，P3-R02 同一理由），
    /// P3-R243 的 [`Renderer::prewarm_gpu_particles`] 让宿主显式把这份钱挪到启动期。
    /// 目标格式取 `self.config.format`（LDR）/`Rgba16Float`（HDR），与后处理离屏纹理、
    /// 热重载重建 LDR 槽位读的是同一个来源。
    gpu_particle_pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    pipeline: wgpu::RenderPipeline,
    /// P3-R04-3：按固定轴缓存的 3D 网格/阴影管线。`Arc` 让绘制侧只借用、不复制；
    /// 键与统计见 [`pipeline_cache`]。
    pipelines: PipelineCache<Arc<wgpu::RenderPipeline>>,
    /// P3-R02/R05：Legacy 网格变体的两组布局（相机 + 材质）。延迟补编译与热重载都要按
    /// **同一个**布局重建管线，所以布局必须留在渲染器上，而不是只活在构造期的局部变量里。
    ///
    /// 只有热重载会读它（延迟补编译在构造期用同一份局部布局），因此与覆盖目录同侧：wasm 不带。
    #[cfg(not(target_arch = "wasm32"))]
    mesh_pipeline_layout: wgpu::PipelineLayout,
    /// P3-R05：shader 源解析层（内置源 + 开发期覆盖目录）。
    ///
    /// 只在原生侧存在：wasm/release 不暴露覆盖入口，文件 IO 分支也不该编进浏览器产物。
    #[cfg(not(target_arch = "wasm32"))]
    shader_library: crate::shader_library::ShaderLibrary,
    /// 最近一次热重载的结果（`None` = 从没重载过）。编辑器状态栏/Console 与验收样例读它。
    #[cfg(not(target_arch = "wasm32"))]
    shader_reload: Option<crate::renderer::shader_hot_reload::ShaderReloadReport>,
    /// 管线里**当前编译着**的源（根名 → 快照），热重载的比较基准。
    ///
    /// 不能拿"覆盖目录此刻解析出什么"当基准：删掉覆盖文件时解析结果已经变成内置源，而画面
    /// 还是旧 shader 画的，两者一比就"没变化"，回退永远不会发生。
    #[cfg(not(target_arch = "wasm32"))]
    shader_committed:
        std::collections::HashMap<&'static str, crate::renderer::shader_hot_reload::ActiveSource>,
    /// 2D 固定槽位（textured / 矢量文字 / 世界 Sprite / 后处理+Tonemap）的管线布局。
    ///
    /// 这几条管线不进 `PipelineCache`（没有变体轴），热重载要按构造期**同一份**描述符重建，
    /// 所以布局得活到重载期；只在原生侧存在，wasm 产物里不留这个字段。
    #[cfg(not(target_arch = "wasm32"))]
    fixed_slot_layouts: crate::renderer::shader_hot_reload::FixedSlotLayouts,
    /// P3-R02：PBR 变体在宿主**首次打开** `RenderSettings.pbr` 时编译，而不是启动期无条件
    /// 编译——旧 3D 场景永远不付这份启动时间，回滚档的像素与耗时都不变。
    ///
    /// P3-R03：PBR 管线布局是相机 + 材质 + 环境三组。Legacy 仍用构造期的两组布局，
    /// 回滚档因此不声明也不绑定环境组。布局留一份在这里正是为了延迟编译；重复打开只编译一次。
    pbr_pipeline_layout: wgpu::PipelineLayout,
    /// P3-R02-4：mask/双面阴影变体也要能延迟补编译，因此阴影布局同样留在渲染器上。
    shadow_pipeline_layout: wgpu::PipelineLayout,
    pbr_pipelines_ready: bool,
    /// P3-R03：环境 uniform（160 B）与 group 2 绑定。未设置环境时写入全零 SH + 1×1 黑 cube，
    /// 绑定关系不随 IBL 开关变化；设置资产后只替换 cubemap 视图并重建绑定组。
    environment_buffer: wgpu::Buffer,
    environment_bind_group: wgpu::BindGroup,
    environment_bind_group_layout: wgpu::BindGroupLayout,
    environment_fallback_view: wgpu::TextureView,
    environment_sampler: wgpu::Sampler,
    environment_view: wgpu::TextureView,
    /// 已上传的预过滤 cubemap；`None` = 镜面 IBL 不可用（SH-only 仍可工作）。
    environment_map: Option<environment::GpuEnvironmentMap>,
    /// 已设置的环境光照；`None` = 本帧 IBL 为 Off（保留 R02 的常数环境光）。
    environment: Option<EnvironmentLighting>,
    camera_bind_group: wgpu::BindGroup,
    /// 四级 CSM 阴影 pass 的相机 bind group（各自引用一份 64B 相机 buffer）。
    shadow_pass_bind_groups: [wgpu::BindGroup; 4],
    /// 每级阴影 pass 的相机 buffer；`[0]` 近级、`[1]` 中级、`[2]` 远级、`[3]` 超远级。
    shadow_pass_camera_buffers: [wgpu::Buffer; 4],
    /// 6 个面分别作为深度附件渲染用的 D2 视图。
    point_shadow_face_views: [wgpu::TextureView; POINT_SHADOW_LAYERS],
    /// 主着色器 group0 的点光阴影 uniform（6 面矩阵 + 位置/远平面，400B）。
    point_shadow_uniform_buffer: wgpu::Buffer,
    point_shadow_camera_buffers: [wgpu::Buffer; POINT_SHADOW_LAYERS],
    point_shadow_bind_groups: [wgpu::BindGroup; POINT_SHADOW_LAYERS],
    depth_view: wgpu::TextureView,
    /// Current depth-view size; rebuilt when the render target changes size.
    depth_size: (u32, u32),
    /// 每级阴影图的单层渲染视图。
    shadow_views: [wgpu::TextureView; 4],
    /// ESM 的 4 个半分辨率层视图；方向光 filtered shadow 打开时才写。
    esm_layer_views: [wgpu::TextureView; 4],
    esm_pipeline: wgpu::RenderPipeline,
    esm_bind_groups: [wgpu::BindGroup; 4],
    /// 本帧实际被写入的深度视图（3D 共用 depth_view；纯世界 Sprite 用独立视图）。
    ssao_depth_view: wgpu::TextureView,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// M79 P1-3c：**逐纹理**采样器表（`滤镜 × 寻址` 全组合，构造时一次建好）。
    ///
    /// 为什么预建而不是惰性缓存：取采样器的那条路（`upload_texture*`）是 `&self`，
    /// 往里写要 `&mut self` 或锁；全组合只有 [`TextureFilter::COUNT`] × [`TextureWrap::COUNT`]
    /// 个、每个几十字节，换来的是"解析采样器"全程不可变、也不随纹理数增长。
    /// 下标 = `filter.index() * TextureWrap::COUNT + wrap.index()`。
    texture_samplers: Vec<wgpu::Sampler>,
    /// 工程根下那份可提交的资源元数据段（`resources.meta`）。`None` = 没登记任何设置。
    resource_meta: Option<rustlua_assets::import_settings::ResourceMeta>,
    /// 只读一次盘（每次 `Renderer` 生命周期内）；改设置后由 `reload_import_settings` 强制重读。
    resource_meta_loaded: bool,
    /// 有槽位的绑定组被**就地换掉**过（设置改了）。2D 保序合并的纪元必须含它，
    /// 否则改设置后同一 `(texture, sdf, bitmap)` 键会继续沿用旧绑定组 = 看不到变化。
    sampling_epoch: u64,
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
    /// 为什么要有这个字段：`construct.rs` 里还有两处存储缓冲（矢量文字的两个绑定、实例环）
    /// 在 `max_storage_buffers_per_shader_stage = 0` 的设备上**建不出来**，而校验错误在 wasm 上
    /// 会把进程 abort。3D 网格/阴影不在此列——它们按 `InstanceMode` 在存储缓冲与 64 字节
    /// uniform 之间选实现，两条路都照常画。这个标志留给渲染侧判矢量文字：`passes.rs` 据此
    /// **不声明**矢量文字通道，并把丢弃的 draw 记进 `dropped_draws`——少画东西而日志里一个字
    /// 都没有，是本仓最不接受的失败。
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
    /// P3-R245：帧路径 GPU 位姿档（默认关）。关时 `frame_3d_draws` 一个字节都不碰。
    gpu_motion_batches: bool,
    /// P3-R250：GPU 位姿烘焙的逐实例距离剔除半径（米，默认 0 = 关）。剔除球心用当帧
    /// 相机世界位置；矩阵同时供主视图/阴影/planar ⇒ 剔除是全链一致的取舍。
    gpu_motion_cull_distance: f32,
    /// P3-R255：逐实例视锥剔除档（默认关）。开档时烘焙内核再把实例保守包围球对主视图
    /// 六面判一次；球整颗在外侧 ⇒ 与距离剔除同一条退化路径（零矩阵 / 不领槽位）。
    gpu_motion_frustum_cull: bool,
    /// P3-R266：远程密度档 `(半径米, 步长)`（默认 `None` = 关）。离相机超过半径的实例
    /// 只保留批次内 `index % 步长 == 0` 的那些；近距离一只不动。这是一条**观感取舍档**
    /// （远区变稀），判据用静态下标 ⇒ 留下的那几只每帧固定，不闪。
    gpu_motion_thin: Option<(f32, u32)>,
    /// P3-R267：第二远带（必须比第一带更远；`None` = 关）。
    gpu_motion_thin_far: Option<(f32, u32)>,
    /// P3-R255：当帧主视图视锥六面（归一化，`[nx, ny, nz, d]` × 6）。关档时恒 `None`；
    /// `render_passes` 每帧从与网格同一份 `main_matrix` 打包，烘焙只读这一份。
    bake_frustum: Option<[[f32; 4]; 6]>,
    /// P3-R250：当帧相机世界位置（`update_3d_uniforms` 每帧写入），供烘焙剔除球心用。
    camera_world_position: Vec3,
    /// P3-R245：宿主在本帧 `render`/`render_into_texture` 前登记的运动批次。按
    /// `Scene3dDrawable::entity` 的 id 认领，没被认领的在帧末丢弃。第三项是求值时刻
    /// （场景时钟秒）——渲染器不持有场景时钟，必须由宿主给。
    staged_motion: Vec<(u64, Arc<InstanceBatch3D>, f32)>,
    /// P4-①：3D 材质绑定组缓存（按本帧 draw 序位）。稳态帧零 `create_bind_group`；
    /// 详见 `arena::MaterialBindKey` 的注释。arena 扩容时靠代次让旧条目失配。
    material_bind_cache: Vec<arena::CachedMaterialBind>,
    material_arena_epoch: u64,
    instance_arena_epoch: u64,
    /// 本帧新建/复用次数与"打过了吗"（诊断用；见 `note_material_bind_frame`）。
    material_bind_created: u64,
    material_bind_reused: u64,
    material_bind_frames: u64,
    material_bind_reported: bool,
    uniform_align: u64,
    storage_align: u64,
    /// GPU geometry + file materials cached by mesh path.
    mesh_cache: HashMap<String, CachedMesh>,
    /// docs/54 §4.4：网格资源身份 → `mesh_cache` 的键。运行时给了句柄就按句柄查（O(1)），
    /// 并且同一网格跨帧恒同键——路径字符串重新解析、或两个不同路径互相顶替，都不会发生。
    mesh_slots: HashMap<AssetHandle, String>,
    /// P3-R166：整张地形的高度场缓存，键是去掉分块字段后的 mesh path。
    /// 同一地形的 16 个分块共享一张贴图，只有 `origin` 不同。
    section_fields: HashMap<String, SectionHeightMap>,
    /// 网格身份缓存命中/未命中次数（诊断/验收用）。
    mesh_identity_hits: u64,
    mesh_identity_misses: u64,
    /// Color textures (base color / emissive, sRGB), indexed by
    /// `GpuMaterial::albedo` / `GpuMaterial::emissive_texture`.
    albedo_textures: Vec<CachedMaterialTexture>,
    /// Data textures (metallic-roughness, linear), indexed by
    /// `GpuMaterial::metallic_roughness`.
    linear_textures: Vec<CachedMaterialTexture>,
    /// 贴图内容 → 槽位。同一份内嵌像素被多个图元/材质引用时只上传一次；
    /// 键含尺寸与色彩空间，sRGB 与线性语义不会互相顶替。
    texture_content_cache: HashMap<(u64, u32, u32, bool, u32), u32>,
    /// `.material` 路径 → 已解析的 GPU 材质。`None` = 加载/解析失败（已告警，
    /// 后续帧不再重复尝试，绘制回退到 glTF 内嵌材质）。
    material_cache: HashMap<String, Option<GpuMaterial>>,
    /// P3-E02：编辑器实时预览（不改盘）覆盖的路径；同一时刻只有一个路径处于预览。
    material_preview_path: Option<String>,
    /// 预览材质最近一次使用的五个贴图路径。路径集合不变时复用已上传的纹理槽，
    /// 拖数值滑杆不做 PNG 解码/上传。
    material_preview_textures: Option<[Option<String>; 5]>,
    material_preview_indices: Option<MaterialTextureIndices>,
    /// Shared sampler for material textures.
    albedo_sampler: wgpu::Sampler,
    /// 1x1 white texture view bound when a material texture is absent.
    white_albedo_view: wgpu::TextureView,
    /// CPU frustum culling of scene drawables (M8-08).
    cull_enabled: bool,
    /// P3-R183：本帧 `frame_3d_draws` 的剔除计数（每次调用重置），随 metrics 上报。
    frame_cull_candidates: u64,
    frame_cull_frustum: u64,
    /// P3-R206 读数补全：本帧被最大绘制距离档剔掉的实例数（已含在 `frame_cull_frustum` 里，
    /// 单列一份只为归因；距离档关闭时恒 0）。
    frame_cull_distance: u64,
    /// P3-R184：开发期归因开关（`--skip-mesh`）：网格路径含该子串的 3D drawable 整个跳过
    /// （在缓存查询与剔除之前）。`None` = 不跳过（默认）。只为 A/B 归因某一类网格的
    /// GPU 成本，不改任何渲染语义；阴影/反射/深度各 pass 复用同一份 `frame_draws`，一并生效。
    skip_mesh_filter: Option<String>,
    /// 本帧被 `skip_mesh_filter` 跳过的 drawable 数（每次调用重置），进周期基线日志。
    frame_skip_mesh: u64,
    /// P3-R252：本帧因「整批都在距离剔除球外」直接不登记 draw 的 GPU 位姿批次 draw 数
    /// （每次调用重置）。像素与"烘焙后零矩阵"一致，省掉的是 53 个阴影 pass/planar/主视图
    /// 各自录一遍这条 draw；剔除档关闭时恒 0。
    frame_gpu_motion_culled: u64,
    /// P3-R254：本帧被整批跳过的 GPU 位姿批次带的实例总数（每次调用重置）。
    /// `frame_gpu_motion_culled` 数的是 draw 条数，这里数的是原本会烘进实例缓冲的那批实例；
    /// 有了它，`cull_candidates − cull_frustum − skipped_instances` 才重新约等于"提交实例"。
    frame_gpu_motion_culled_instances: u64,
    /// P3-R185：不透明 draw 按到相机距离前到后排序（early-Z 拒绝被挡像素）。
    /// `false`（`--no-depth-sort` 回滚档）= 保持声明/合批顺序。Blend 队列不受影响。
    depth_sort_enabled: bool,
    /// P3-R191：平面反射按镜像视锥剔除（`--no-planar-cull` 回滚档，A/B 用）。
    planar_cull_enabled: bool,
    /// P3-R194：点光阴影逐面方向锥剔除（`--no-point-face-cull` 回滚档，A/B 用）。
    point_face_cull_enabled: bool,
    /// P3-R196：平面反射目标的分辨率除数（2 = 半分辨率默认；4 = 四分之一档）。
    planar_reflection_divisor: u32,
    /// P3-R206：3D 最大绘制距离（米，0 = 关闭）。超出者按包围球边缘判、并入
    /// `cull_frustum` 计数（低端机的第一级植被 LOD：硬截断，未做渐隐）。
    max_draw_distance: f32,
    /// P3-R206 分类档：只对网格路径含该子串的 drawable 生效（`None` = 全体，默认）。
    /// 植被 LOD 的第一级就是「只掐草」，不能把蚁/地形/道具一起掐掉。
    max_draw_distance_filter: Option<String>,
    /// P3-R248：方向光 CSM 链的**分类排除**档（样本名或网格路径含该子串的 drawable
    /// 不投影；`None` = 不排除，默认）。与距离档同一"按类别掐"的形状：6 万只蚂蚁的阴影
    /// 在 2048² 覆盖 480 m 时一个纹素约 0.23 m，比蚂蚁本身还大 ⇒ 留着只是白付几何。
    /// 只影响阴影链，主视图照画（与 `max_draw_distance` 的"两个 pass 一起掐"不同）。
    shadow_caster_skip: Option<String>,
    /// P3-R265：点光 6 面链的**专用**分类排除档（`None` = 不排除，默认）。与
    /// `shadow_caster_skip` 的区别是作用域：后者两链一起掐，这一档只掐点光链，
    /// 方向光 CSM 的 crowd 阴影保留。P3-R263 的归因显示 shadow 的 147 ms 里有相当
    /// 一部分是最近几盏灯照到的 crowd 面，而 CSM 的 crowd 阴影在 2048²/480 m 下
    /// 一个纹素比蚂蚁还大 ⇒ 值得给"掐点光、留 CSM"一个单独的档位。
    point_shadow_caster_skip: Option<String>,
    /// P3-R256：阴影投射者的**距离预算**（米，`0.0` = 关闭，默认）。离相机超过该距离的
    /// drawable 不进阴影链（方向光 CSM 与点光 6 面都掐），主视图/平面反射照旧。与 R248 的
    /// 分类档同属观感取舍，但按几何位置而不是类别：远处的阴影本来只值几个纹素。
    /// 判据与 `max_draw_distance` 同形——包围盒到相机的最近距离（不是中心距离）。
    shadow_caster_max_distance: f32,
    /// P3-R207：深度加权折射增益（uv / 米 / 掠射角）。`0.0` = 关闭（默认），
    /// 此时 Tonemap 仍用旧的固定偏移；非有限值同样归一为关闭。
    water_refraction_gain: f32,
    /// P3-R208：水下视角的界面替换强度（0..=1）。`0.0` = 关闭（默认），此时相机入水
    /// 只有 P3-R173 的整帧吸收；非有限或负值同样归一为关闭。
    submerged_view_gain: f32,
    /// P3-R241：水面折射的屏幕空间步进次数（`0` = 关闭（默认）= R207 的偏移近似，
    /// 像素逐字节不变；`1..=32` = 沿 Snell 折射方向在水下步进找几何交点）。
    water_refract_steps: u32,
    /// P3-R209：剖切口的实体封口深度（米）。`0.0` = 关闭（默认），双面网格的切口只画
    /// P3-R54 那 0.08 m 薄带；非有限或负值同样归一为关闭。
    clip_cap_depth: f32,
    /// P3-R211：封口要不要也吃到**单面**网格（`--clip-cap-both-sides`）。`false`（默认）=
    /// 主网格通道按材质自身的双面档选管线，单面 glb 壳的背面在第一遍就被硬件剔除 ⇒
    /// 封口分支没有像素可画；`true` = 仅当本帧真挂了会话裁剪平面时，主网格通道改用双面
    /// 变体，把背面放回来让剖面切得开。不新建管线：双面 opaque 变体已由 P3-R170 的反射
    /// 预热建好，命中缓存；没有裁剪面时选键逐字回到旧形状 ⇒ 像素与代价都不变。
    clip_cap_both_sides: bool,
    /// P3-R197：开发期全局覆盖——所有水面强制走廉价光照档（`--water-cheap`，A/B 用）。
    water_cheap_override: bool,
    /// P3-R203：开发期全局覆盖——所有地形强制走廉价细节档（`--terrain-cheap`，A/B 用）。
    terrain_cheap_override: bool,
    /// P3-R198：方向光 CSM 阴影图边长（默认 2048；512/1024 为低端档）。构建期决定，
    /// 纹理与绑定组在 `new_async` 里一次建好，运行期不重建。
    shadow_resolution: u32,
    /// P3-R199：方向光 CSM 实际渲染的级联层数（1..=4，默认 4）。未渲染的远级联在构建期
    /// 清成「全亮」（深度 1.0）⇒ 只失去该距离的阴影，不会被旧内容误压暗。
    shadow_cascade_limit: u32,
    /// P3-R200：点光 cube 阴影面的边长（默认 1024；512 为低端档，8×6 面 192→48 MiB）。
    /// 构建期决定，运行期不重建。
    point_shadow_resolution: u32,
    /// P3-R202：阴影盏数预算（1..=8）——只给相机焦点最近的 N 盏点光录 6 面。
    /// 运行期可改，只影响每帧 uniform 与 pass 录制，不重建纹理/绑定。
    point_shadow_light_budget: u32,
    /// Grouping of same mesh/material draws into instanced calls (M8-08).
    instancing_enabled: bool,
    /// Frames rendered since startup, for periodic baseline logging.
    frame_count: u64,
    /// Monotonic render clock. Only the `w` lane of the camera position is used
    /// (water ripples); gameplay time stays authoritative in the 10 Hz sim.
    clock: web_time::Instant,
    /// P3-R236：时间锁。着色器时间 uniform 全部走 `clock_secs()`，钉住后同一场景的
    /// 任意两帧逐像素可比（生产路径恒为 `None` = 渲染钟）。
    clock_frozen_secs: Option<f32>,
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
    /// E06-3：上一帧逐通道 GPU 时间（与 `gpu_frame_us` 来自同一次提交）。
    last_gpu_pass_report: GpuPassReport,
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
    /// Tonemap/SSAO 专用布局：额外深度纹理与逆视图投影。
    tonemap_layout: wgpu::BindGroupLayout,
    /// 场景离屏颜色纹理（打开后处理时才创建）。视图单独存一份，好让每帧"取出 → 用完放回"，
    /// 从而不必在 `render_passes` 里同时持有 `&self` 与 `&mut self`。
    post_process_texture: Option<wgpu::Texture>,
    post_process_scene_view: Option<wgpu::TextureView>,
    post_process_size: (u32, u32),
    /// P3-R01：工程/场景级渲染开关（HDR 是第一个消费者）。默认全关 ⇒ 与收敛前逐字节相同。
    render_settings: RenderSettings,
    /// HDR 因"曝光归属冲突"等原因未能启用时只告警一次（每帧刷屏会淹掉真正的信号）。
    hdr_warning_logged: bool,
    /// HDR 场景颜色目标（`Rgba16Float`）。只在真正需要 HDR 的帧按需创建；纯 2D 帧不分配。
    hdr_scene_texture: Option<wgpu::Texture>,
    hdr_scene_view: Option<wgpu::TextureView>,
    hdr_scene_size: (u32, u32),
    hdr_world_sprite_pipeline: Option<wgpu::RenderPipeline>,
    hdr_world_sprite_opaque_pipeline: Option<wgpu::RenderPipeline>,
    /// Tonemap 全屏管线（输入 HDR 场景、输出 LDR 目标）。能力不足时为 `None`。
    tonemap_pipeline: Option<wgpu::RenderPipeline>,
    tonemap_params: wgpu::Buffer,
    /// P3-R166：没有 `sect=1` 地形时 Tonemap 绑定的 1×1 占位高度图。
    section_height_white: wgpu::TextureView,
    /// 半分辨率 SSAO 生成 pass 与目标；Tonemap 只读这张 AO 图。
    ssao_pipeline: wgpu::RenderPipeline,
    ssao_bind_group_layout: wgpu::BindGroupLayout,
    ssao_params: wgpu::Buffer,
    ssao_blur_pipeline: wgpu::RenderPipeline,
    ssao_blur_bind_group_layout: wgpu::BindGroupLayout,
    ssao_blur_params: wgpu::Buffer,
    ssao_raw_view: wgpu::TextureView,
    ssao_view: wgpu::TextureView,
    ssao_size: (u32, u32),
    /// 半分辨率 SSR：rgb=反射颜色，a=命中。
    ssr_pipeline: wgpu::RenderPipeline,
    ssr_bind_group_layout: wgpu::BindGroupLayout,
    ssr_params: wgpu::Buffer,
    ssr_blur_pipeline: wgpu::RenderPipeline,
    ssr_blur_bind_group_layout: wgpu::BindGroupLayout,
    ssr_blur_params: wgpu::Buffer,
    ssr_raw_view: wgpu::TextureView,
    ssr_view: wgpu::TextureView,
    ssr_history_view: wgpu::TextureView,
    ssr_texture: wgpu::Texture,
    ssr_history_texture: wgpu::Texture,
    ssr_prev_view_projection: Option<Mat4>,
    /// P3-R174：上一帧 Tonemap 用的视图投影（相机运动模糊的重投影用）。与 SSR 的那份分开：
    /// SSR 关掉时它不更新，而运动模糊要跟着每一帧走。
    tonemap_prev_view_projection: Option<Mat4>,
    ssr_size: (u32, u32),
    /// P3-R170：水面平面反射的离屏颜色目标（半分辨率 `Rgba16Float`）+ 自己的深度附件。
    /// 未启用时绑定 1×1 透明占位图，Tonemap 的绑定组形状因此恒定。
    planar_reflection_texture: wgpu::Texture,
    planar_reflection_view: wgpu::TextureView,
    planar_reflection_depth: wgpu::TextureView,
    planar_reflection_size: (u32, u32),
    /// 镜像相机 uniform（与主相机同一布局，160B）与它的绑定组。
    planar_camera_buffer: wgpu::Buffer,
    planar_camera_bind_group: wgpu::BindGroup,
}

/// 光照 uniform 的浮点字数：方向光 12（direction / color_intensity / params）+ 点光 8×8。
/// 点光阴影每面边长；前 `POINT_SHADOW_LIGHTS` 盏各 6 面，共 `POINT_SHADOW_LAYERS` 层。
pub(super) const POINT_SHADOW_SIZE: u32 = 1024;
const POINT_SHADOW_NEAR: f32 = 0.1;
pub(super) const POINT_SHADOW_LIGHTS: usize = 8;
pub(super) const POINT_SHADOW_LAYERS: usize = POINT_SHADOW_LIGHTS * 6;
/// 6 面 × 8 盏矩阵（48×64B）+ 8 组位置/远平面（8×16B）= 3200B。
pub(super) const POINT_SHADOW_UNIFORM_BYTES: usize =
    POINT_SHADOW_LAYERS * 64 + POINT_SHADOW_LIGHTS * 16;

/// ESM filtered shadow 的边长：与方向光 CSM 同分辨率，4 层。
/// `R16Float` 每层 2 B/px ⇒ 4 × 2048² × 2 B = 32 MiB。
pub(super) const ESM_SIZE: u32 = 2048;

/// P3-R198：方向光 CSM 的缺省边长。`Renderer::new_with_shadow_resolution` 可降档，
/// 默认值保持不变 ⇒ 默认路径与旧版逐字节一致。
pub(super) const DEFAULT_SHADOW_RESOLUTION: u32 = 2048;

/// P3-R199：方向光 CSM 缺省级联层数。默认渲染四级 ⇒ 与旧版像素一致。
pub(super) const DEFAULT_SHADOW_CASCADE_LIMIT: u32 = 4;

/// P3-R200：点光 cube 阴影面的缺省边长。默认值保持不变 ⇒ 默认路径逐字节一致。
pub(super) const DEFAULT_POINT_SHADOW_RESOLUTION: u32 = 1024;
pub(super) const ESM_LAYERS: usize = 4;

const LIGHT_UNIFORM_WORDS: usize = 12 + MAX_POINT_LIGHTS * 8;
/// 对应字节数。`construct.rs` 的绑定尺寸与着色器结构必须同时等于它。
pub(super) const LIGHT_UNIFORM_BYTES: u64 = (LIGHT_UNIFORM_WORDS * 4) as u64;

/// PBR 材质 uniform 的 `vec4` 行数（P3-R02 起 6 行，P3-R162–165 涨到 10 行）。
///
/// 三处必须同步：`mesh3d_pbr.wgsl` 的 `struct Material`、`arena.rs` 的 CPU 打包、
/// `construct.rs` 的绑定布局 `min_binding_size`。少一行不会编译失败，只会让 GPU
/// 读到上一行的值或零——画面表现为"某瓣不生效"，像素门未必指得出原因，
/// 所以由 `material_uniform_rows_match_the_wgsl_struct` 把三者钉死。
pub(super) const MATERIAL_UNIFORM_ROWS: usize = 10;
pub(super) const MATERIAL_UNIFORM_BYTES: u64 = (MATERIAL_UNIFORM_ROWS * 16) as u64;

/// Tonemap uniform 的浮点字数（P3-R166 起 96；P3-R170 的平面反射再加 4 = 100；
/// P3-R171 的水体吸收再加 4 = 104、P3-R172 的水体散射再加 4 = 108、
/// P3-R173 的相机入水体积吸收再加 4 = 112；P3-R174 的运动模糊再加 mat4 + vec4 = 132；
/// P3-R208 的水下界面再加 4 = 136；P3-R241 的水面折射步进再加 4 = 140：
/// 8 标量 + 4×mat4 + 17 个 vec4 槽）。
///
/// 三处必须同步：`tonemap.wgsl` 的 `struct Params`、`passes.rs` 的 `params` 数组、
/// `construct.rs` 的绑定布局 `min_binding_size`/缓冲区大小。少写一个槽不会编译失败，
/// 只会让着色器读到零——剖面封口这类"某帧才开"的分支会静默不生效，因此由
/// `tonemap_uniform_layout_matches_the_wgsl_struct` 按 WGSL 字段逐项算偏移钉死。
pub(crate) const TONEMAP_UNIFORM_FLOATS: usize = 140;
pub(crate) const TONEMAP_UNIFORM_BYTES: u64 = (TONEMAP_UNIFORM_FLOATS * 4) as u64;

/// 水面平面反射的镜像视图投影（P3-R170）。
///
/// 世界先关于 `y = level` 平面镜像（`R`），再走主视图投影：`clip = (VP·R)·p`。
/// 镜像图因此**已经是屏幕朝向**：水面像素的反射内容就落在该像素自己的 uv 上，
/// Tonemap 直接按当前 uv 采样（不能再补一次 `1 - u`）。
///
/// 为什么不再左乘 `F = diag(-1,1,1,1)`：那一步会把裁剪空间 x 取反，连带把屏幕绕序
/// 翻过来。`det(F)·det(R) = +1` 只保证 3×3 线性部分同号，**不保证透视除法之后的
/// 屏幕绕序**——实测镜像四边形在 `F·VP·R` 下全被背面剔除（P3-R170 像素门红：
/// draws=1、覆盖 0；去掉 `F` 后同一帧覆盖 244 像素）。`det3` 同号那条离线门因此
/// 是假绿，已删除。镜像是反向变换，屏幕绕序对一部分几何翻转、对另一部分保持，
/// 不存在单一剔除模式能服务所有几何 ⇒ 反射 pass 改用双面变体（`cull_mode: None`），
/// 覆盖由 `planar_reflection_only_draws_geometry_above_the_water` 的像素回读兜住。
///
/// 镜像平面上的点满足 `R·p = p`，所以水面像素在镜像图里落在同一屏幕坐标，
/// 这正是投影采样能用当前 uv 的前提。
pub(crate) fn planar_reflection_view_projection(main: &Mat4, level: f32) -> Mat4 {
    let mirror = Mat4 {
        columns: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, -1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 2.0 * level, 0.0, 1.0],
        ],
    };
    *main * mirror
}

/// 打包一帧的光照 uniform。
///
/// 布局（与 `mesh3d_pbr.wgsl` 的 `Light` 一一对应）：
/// `direction` / `color_intensity` / `params = [count, soft_shadow, 0, 0]`，
/// 随后 8 组 `[position.xyz, range]` + `[color.rgb, intensity]`，未用的槽位补零。
fn light_uniform_bytes(
    light: Light,
    points: &[PointLight],
    soft_shadows: bool,
    filtered_shadows: bool,
    point_shadow: Option<(usize, f32)>,
) -> Vec<u8> {
    let (direction, color_intensity) = light.uniform();
    let mut words = [0.0_f32; LIGHT_UNIFORM_WORDS];
    words[0..4].copy_from_slice(&direction);
    words[4..8].copy_from_slice(&color_intensity);
    words[8] = points.len().min(MAX_POINT_LIGHTS) as f32;
    words[9] = if soft_shadows { 1.0 } else { 0.0 };
    // z = 方向光 ESM 开关；w = 点光远平面（旧的 slot 索引没有消费者）。
    words[10] = if filtered_shadows { 1.0 } else { 0.0 };
    if let Some((_index, far)) = point_shadow {
        words[11] = far;
    }
    for (index, point) in points.iter().take(MAX_POINT_LIGHTS).enumerate() {
        let (position_range, tint) = point.uniform();
        let base = 12 + index * 8;
        words[base..base + 4].copy_from_slice(&position_range);
        words[base + 4..base + 8].copy_from_slice(&tint);
    }
    bytemuck::cast_slice(&words).to_vec()
}

impl Renderer {
    /// Rewrites the camera, light, and light-space uniform buffers from the
    /// active scene's camera and directional light.
    ///
    /// `view_projection` 是宿主声明的世界视图矩阵（docs/54 §8-③）。有声明就用声明的那一份，
    /// 没有才回退到 `Scene3d.view`——那是**同一次视口烘焙**的结果（相机拆分批 7），因此回退
    /// 路径与声明路径不可能给出两个不同的比例。
    fn update_3d_uniforms(&mut self, scene: &Scene3d, view_projection: Option<Mat4>) {
        // P3-R250：烘焙剔除球心用当帧相机位置。放在早退之前——视图矩阵缺失的帧也要更新，
        // 否则上一帧的位置会被当成这一帧的球心。
        self.camera_world_position = scene.camera.position;
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
        // P3-R02：相机 uniform 扩成「视图投影 + 世界位置」。位置只在 PBR 片元的视线向量里
        // 用到；旧路径的着色器结构仍是 64 字节，读同一个缓冲区的前 64 字节，像素不变。
        let (ripple, ripple_extra) = self.next_ripples();
        let camera_uniform = camera_uniform(
            &matrix,
            scene.camera.position,
            self.clock_secs(),
            scene.camera.clip_plane,
            self.cloud_coverage,
            self.cloud_speed,
            self.cloud_base,
            self.cloud_shadow_strength,
            self.water_tint,
            self.surface_cover,
            ripple,
            ripple_extra,
            self.clip_cap_depth,
        );
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&camera_uniform),
        );
        if view_projection.is_none() {
            // 合法回滚档：宿主没声明视图时回退到场景提取时烘焙的视图。trace 级别，不刷屏。
            tracing::trace!(target: "render", "[render] 3D view matrix fell back to Scene3d.view");
        }
        self.write_light_uniforms(scene);
    }

    /// 光照与阴影 uniform（与相机矩阵分开写，缺相机时也要更新）。
    fn write_light_uniforms(&mut self, scene: &Scene3d) {
        let point_shadow = self.render_settings.point_shadows.then(|| {
            scene
                .point_lights
                .first()
                .map(|point| (0usize, point_shadow_far(point.range)))
        });
        let point_shadow = point_shadow.flatten();
        self.queue.write_buffer(
            &self.light_buffer,
            0,
            &light_uniform_bytes(
                scene.light,
                &scene.point_lights,
                self.render_settings.soft_shadows,
                self.render_settings.filtered_shadows,
                point_shadow,
            ),
        );
        // P3-R202：预算内只给最近的 N 盏点光写 6 面矩阵；其余槽位保持全零，
        // 着色器用 params.w>0 判定有效性 ⇒ 预算外的那几盏自动无阴影。
        let mut bytes = vec![0u8; POINT_SHADOW_UNIFORM_BYTES];
        if self.render_settings.point_shadows {
            let selection =
                point_shadow_light_indices(scene, self.point_shadow_light_budget as usize);
            for (slot, &index) in selection.iter().enumerate() {
                let point = &scene.point_lights[index];
                let far = point_shadow_far(point.range);
                let cameras = point_shadow_view_projections(point.position, far);
                let matrix_base = slot * 6 * 64;
                for (face, camera) in cameras.iter().enumerate() {
                    let offset = matrix_base + face * 64;
                    bytes[offset..offset + 64]
                        .copy_from_slice(bytemuck::cast_slice(&camera.columns));
                    self.queue.write_buffer(
                        &self.point_shadow_camera_buffers[slot * 6 + face],
                        0,
                        bytemuck::cast_slice(&camera.columns),
                    );
                }
                let params_offset = POINT_SHADOW_LAYERS * 64 + slot * 16;
                bytes[params_offset..params_offset + 16].copy_from_slice(bytemuck::cast_slice(&[
                    point.position.x,
                    point.position.y,
                    point.position.z,
                    far,
                ]));
            }
        }
        self.queue
            .write_buffer(&self.point_shadow_uniform_buffer, 0, &bytes);
        // 阴影跟着相机焦点走：四级阴影图都以 target 为中心，近/中/远/超远半宽随距离伸缩。
        let distance = (scene.camera.position - scene.camera.target).length();
        let cascades = shadow_view_projections(
            scene.light,
            scene.camera.target,
            distance,
            self.shadow_resolution,
        );
        let matrices: Vec<[[f32; 4]; 4]> = cascades.iter().map(|matrix| matrix.columns).collect();
        self.queue.write_buffer(
            &self.shadow_camera_buffer,
            0,
            bytemuck::cast_slice(&matrices.concat()),
        );
        for (index, matrix) in cascades.iter().enumerate() {
            self.queue.write_buffer(
                &self.shadow_pass_camera_buffers[index],
                0,
                bytemuck::cast_slice(&matrix.columns),
            );
        }
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

    /// P3-R184：开发期归因开关（`--skip-mesh <子串>`）：网格路径含该子串的 3D drawable
    /// 整个跳过。`None`（默认，也是回滚档）= 不跳过，行为逐字节不变。
    pub fn set_skip_mesh_filter(&mut self, filter: Option<String>) {
        self.skip_mesh_filter = filter;
    }

    /// P3-R185：开关不透明前到后排序。`true`（默认）= 排序；`false` = 回滚到
    /// 声明/合批顺序（A/B 对照档）。不透明渲染与顺序无关，两档像素一致。
    pub fn set_depth_sort_enabled(&mut self, enabled: bool) {
        self.depth_sort_enabled = enabled;
    }

    /// P3-R191：平面反射逐 draw 剔除开关（`true` 默认；`false` = A/B 回滚档）。
    pub fn set_planar_cull_enabled(&mut self, enabled: bool) {
        self.planar_cull_enabled = enabled;
    }

    /// P3-R194：点光阴影逐面剔除开关（`true` 默认；`false` = A/B 回滚档）。
    pub fn set_point_face_cull_enabled(&mut self, enabled: bool) {
        self.point_face_cull_enabled = enabled;
    }

    /// P3-R196：平面反射目标分辨率除数（默认 2 = 半分辨率）。4 = 四分之一档（更省、更糊），
    /// 1 = 全分辨率（最贵、最锐）。只在下一帧目标重建时生效，越界值会被夹到 1..=8。
    pub fn set_planar_reflection_divisor(&mut self, divisor: u32) {
        self.planar_reflection_divisor = divisor.clamp(1, 8);
    }

    /// P3-R206：3D 最大绘制距离（米；0 或非正值 = 关闭）。只影响绘制，不改场景数据；
    /// 判定用包围球边缘（`圆心距离 − 半径`），近处大物体不会被"圆心远"误杀。
    pub fn set_max_draw_distance(&mut self, meters: f32) {
        self.max_draw_distance = if meters.is_finite() && meters > 0.0 {
            meters
        } else {
            0.0
        };
    }

    /// P3-R206 分类档：把最大绘制距离限制到网格路径含 `filter` 子串的 drawable
    /// （`None` = 全体，默认）。只影响哪些实例参与距离剔除，不改距离值本身。
    pub fn set_max_draw_distance_filter(&mut self, filter: Option<String>) {
        self.max_draw_distance_filter = filter;
    }

    /// 当前生效的最大绘制距离（米；0 = 关闭，已按 setter 的归一规则处理）。
    #[must_use]
    pub fn max_draw_distance(&self) -> f32 {
        self.max_draw_distance
    }

    /// 当前生效的距离档分类子串（`None` = 全体）。
    #[must_use]
    pub fn max_draw_distance_filter(&self) -> Option<&str> {
        self.max_draw_distance_filter.as_deref()
    }

    /// P3-R248：方向光 CSM 链的分类排除档（网格路径含该子串的 drawable 不投影；
    /// `None` = 不排除，默认）。只影响阴影链——主视图/平面反射照旧，因此画面会丢阴影，
    /// 是一条**观感取舍档**（"小到纹素盖不住的批次不投影"），不是无损剔除。
    pub fn set_shadow_caster_skip(&mut self, filter: Option<String>) {
        self.shadow_caster_skip = filter.filter(|value| !value.is_empty());
    }

    /// 当前生效的阴影投射排除子串（`None` = 不排除）。
    #[must_use]
    pub fn shadow_caster_skip(&self) -> Option<&str> {
        self.shadow_caster_skip.as_deref()
    }

    /// P3-R265：点光 6 面链的专用分类排除档（样本名或网格路径含该子串的 drawable 不进
    /// 点光链；`None` = 不排除，默认）。只影响点光链——方向光 CSM、主视图、平面反射照旧，
    /// 属观感取舍档（远处/小物件的点光阴影），不是无损剔除。
    pub fn set_point_shadow_caster_skip(&mut self, filter: Option<String>) {
        self.point_shadow_caster_skip = filter.filter(|value| !value.is_empty());
    }

    /// 当前生效的点光链投射排除子串（`None` = 不排除）。
    #[must_use]
    pub fn point_shadow_caster_skip(&self) -> Option<&str> {
        self.point_shadow_caster_skip.as_deref()
    }

    /// P3-R256：阴影投射者的距离预算（米；0 或非正值 = 关闭）。只影响阴影链——
    /// 主视图/平面反射照旧，因此画面会丢远处物体的阴影，是一条**观感取舍档**。
    pub fn set_shadow_caster_max_distance(&mut self, meters: f32) {
        self.shadow_caster_max_distance = if meters.is_finite() && meters > 0.0 {
            meters
        } else {
            0.0
        };
    }

    /// 当前生效的阴影投射距离预算（米；0 = 关闭，已按 setter 的归一规则处理）。
    #[must_use]
    pub fn shadow_caster_max_distance(&self) -> f32 {
        self.shadow_caster_max_distance
    }

    /// P3-R207：深度加权折射增益。`0.0` = 关闭（默认，走旧的固定偏移），
    /// 非有限或负值同样归一为关闭。
    pub fn set_water_refraction_gain(&mut self, gain: f32) {
        self.water_refraction_gain = if gain.is_finite() && gain > 0.0 {
            gain
        } else {
            0.0
        };
    }

    /// P3-R236：把着色器时间钉在给定秒数（`None` = 回到渲染钟 = 旧行为）。
    ///
    /// 水波相位、云、天空反射、后期颗粒都由 `clock.elapsed()` 驱动，不钉住的话
    /// 「同一场景两次运行」的像素差里混着相位差，逐像素对账无从谈起。
    /// 非有限的秒数一律拒绝（保持原值）；返回是否生效。
    pub fn set_frozen_clock_secs(&mut self, secs: Option<f32>) -> bool {
        match secs {
            Some(value) if value.is_finite() => {
                self.clock_frozen_secs = Some(value);
                true
            }
            Some(_) => false,
            None => {
                self.clock_frozen_secs = None;
                true
            }
        }
    }

    /// 测试专用入口（生产路径用 [`Self::set_frozen_clock_secs`]）。
    #[cfg(test)]
    pub fn freeze_clock_for_tests(&mut self, secs: f32) {
        self.set_frozen_clock_secs(Some(secs));
    }

    /// 当前生效的深度加权折射增益（0 = 关闭）。
    #[must_use]
    pub fn water_refraction_gain(&self) -> f32 {
        self.water_refraction_gain
    }

    /// P3-R208：水下视角的界面替换强度。`0.0` = 关闭（默认，只有 P3-R173 的吸收），
    /// 1.0 = 界面像素完全换成 Snell 窗口/全反射镜像；非有限或负值归一为关闭，
    /// 大于 1 截断为 1（超过 1 的混合权重在 `mix` 里会反色，不是更强的效果）。
    pub fn set_submerged_view_gain(&mut self, gain: f32) {
        self.submerged_view_gain = if gain.is_finite() && gain > 0.0 {
            gain.min(1.0)
        } else {
            0.0
        };
    }

    /// 当前生效的水下界面强度（0 = 关闭）。
    #[must_use]
    pub fn submerged_view_gain(&self) -> f32 {
        self.submerged_view_gain
    }

    /// P3-R241：水面折射的屏幕空间步进次数。`0` = 关闭（默认，走 R207 的偏移近似，
    /// 像素逐字节不变）；超过 32 截到 32（再多也只在水柱里多采几次并可能钻出屏幕）。
    /// 这条档只改**取样位置**，不改吸收/散射权重；相机在水下时它不生效（水面求交
    /// 在水下没有前向交点，着色器自动退回偏移落点）。
    pub fn set_water_refract_steps(&mut self, steps: u32) {
        self.water_refract_steps = steps.min(32);
    }

    /// 当前生效的水面折射步进次数（0 = 关闭）。
    #[must_use]
    pub fn water_refract_steps(&self) -> u32 {
        self.water_refract_steps
    }

    /// P3-R209：剖切口向内多深仍然画实体截面（米）。`0.0` = 关闭（默认，只有 P3-R54 的
    /// 0.08 m 薄带，像素逐字节不变）；非有限或负值归一为关闭。深度只在会话裁剪平面
    /// 生效时才有意义，水平面/反射那两条平面不吃这条档。
    pub fn set_clip_cap_depth(&mut self, depth: f32) {
        self.clip_cap_depth = if depth.is_finite() && depth > 0.0 {
            depth
        } else {
            0.0
        };
    }

    /// 当前生效的剖面封口深度（0 = 关闭 = 旧的 0.08 m 薄带）。
    #[must_use]
    pub fn clip_cap_depth(&self) -> f32 {
        self.clip_cap_depth
    }

    /// P3-R211：封口是否也吃到单面网格。`false`（默认）= 只有双面网格（水面、带裙边的
    /// 地形、`double_sided` 材质）的切口能画出实体截面；`true` = 有会话裁剪平面时，主网格
    /// 通道对**所有**网格改用双面变体，于是单面 glb 壳（巢丘、道具）也被切成实心剖面。
    /// 没有裁剪面的帧不受影响；阴影/平面反射通道也不吃这条档。
    pub fn set_clip_cap_both_sides(&mut self, enabled: bool) {
        self.clip_cap_both_sides = enabled;
    }

    /// 当前生效的「封口吃到单面网格」档。
    #[must_use]
    pub fn clip_cap_both_sides(&self) -> bool {
        self.clip_cap_both_sides
    }

    /// P3-R197：把全部水面强制切到廉价光照档（A/B 用；`false` = 只按 spec 的 `cheap=1`）。
    pub fn set_water_cheap_override(&mut self, enabled: bool) {
        self.water_cheap_override = enabled;
    }

    /// P3-R203：把全部地形强制切到廉价细节档（A/B 用；`false` = 只按 spec 的 `cheap=1`）。
    pub fn set_terrain_cheap_override(&mut self, enabled: bool) {
        self.terrain_cheap_override = enabled;
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

    /// P3-R01：设置工程/场景级渲染开关（HDR/PBR/IBL/变体）。
    ///
    /// 默认（不调用）是全关：纯 2D 与旧 3D 路径的像素逐字节不变。开关变化时把"只告警一次"
    /// 的标记复位，新的降级原因因此会重新出现在日志里。
    pub fn set_render_settings(&mut self, settings: RenderSettings) {
        if self.render_settings != settings {
            self.render_settings = settings;
            self.hdr_warning_logged = false;
        }
        if settings.pbr {
            // 在宿主显式打开 PBR 的这一刻编译变体（通常发生在首帧之前），而不是等第一次
            // 3D draw 时在通道里编译。重复调用是空操作，统计里的变体数不会涨。
            self.ensure_pbr_pipelines();
        }
    }

    /// 设置运行期云量（0..=1）；返回是否接受。P3-R80。
    pub fn set_cloud_coverage(&mut self, coverage: f32) -> bool {
        if !coverage.is_finite() || !(0.0..=1.0).contains(&coverage) {
            return false;
        }
        self.cloud_coverage = coverage;
        true
    }

    /// 当前运行期云量。
    #[must_use]
    pub const fn cloud_coverage(&self) -> f32 {
        self.cloud_coverage
    }

    /// 设置云层漂移速度倍率（0..=4）；返回是否接受。
    pub fn set_cloud_speed(&mut self, speed: f32) -> bool {
        if !speed.is_finite() || !(0.0..=4.0).contains(&speed) {
            return false;
        }
        self.cloud_speed = speed;
        true
    }

    /// 当前云层速度倍率。
    #[must_use]
    pub const fn cloud_speed(&self) -> f32 {
        self.cloud_speed
    }

    /// 设置云底高度（世界 y，40..=200）；返回是否接受。
    pub fn set_cloud_base(&mut self, base: f32) -> bool {
        if !base.is_finite() || !(40.0..=200.0).contains(&base) {
            return false;
        }
        self.cloud_base = base;
        true
    }

    /// 当前云底高度。
    #[must_use]
    pub const fn cloud_base(&self) -> f32 {
        self.cloud_base
    }

    /// 设置云影强度（0..=0.6）；返回是否接受。
    pub fn set_cloud_shadow_strength(&mut self, strength: f32) -> bool {
        if !strength.is_finite() || !(0.0..=0.6).contains(&strength) {
            return false;
        }
        self.cloud_shadow_strength = strength;
        true
    }

    /// 当前云影强度。
    #[must_use]
    pub const fn cloud_shadow_strength(&self) -> f32 {
        self.cloud_shadow_strength
    }

    /// 设置闪电屏幕闪光（0..=1）；返回是否接受。
    pub fn set_flash(&mut self, intensity: f32) -> bool {
        if !intensity.is_finite() || !(0.0..=1.0).contains(&intensity) {
            return false;
        }
        self.flash_intensity = intensity;
        true
    }

    /// 当前闪电闪光强度。
    #[must_use]
    pub const fn flash_intensity(&self) -> f32 {
        self.flash_intensity
    }

    /// 设置水体染色 rgb + 浑浊度 w（各 0..=1）；返回是否接受。
    pub fn set_water_tint(&mut self, tint: [f32; 4]) -> bool {
        if tint
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return false;
        }
        self.water_tint = tint;
        true
    }

    /// 当前水体染色。
    #[must_use]
    pub const fn water_tint(&self) -> [f32; 4] {
        self.water_tint
    }

    /// 设置地面覆盖 [积雪, 湿润]（各 0..=1）；返回是否接受。
    pub fn set_surface_cover(&mut self, cover: [f32; 2]) -> bool {
        if cover
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return false;
        }
        self.surface_cover = cover;
        true
    }

    /// 当前地面覆盖。
    #[must_use]
    pub const fn surface_cover(&self) -> [f32; 2] {
        self.surface_cover
    }

    /// 设置曝光偏移（EV，-4..=4）；返回是否接受。
    pub fn set_exposure_offset(&mut self, ev: f32) -> bool {
        if !ev.is_finite() || !(-4.0..=4.0).contains(&ev) {
            return false;
        }
        self.exposure_offset = ev;
        true
    }

    /// 当前曝光偏移（EV）。
    #[must_use]
    pub const fn exposure_offset(&self) -> f32 {
        self.exposure_offset
    }

    /// 着色器时间 uniform 的唯一取值点（P3-R207）。生产路径就是渲染钟；
    /// P3-R236 的 `set_frozen_clock_secs` 钉住它（宿主做逐像素对账时用）。
    #[must_use]
    pub(crate) fn clock_secs(&self) -> f32 {
        if let Some(secs) = self.clock_frozen_secs {
            return secs;
        }
        self.clock.elapsed().as_secs_f32()
    }

    /// P3-R135：在世界 (x,z) 触发一圈水面涟漪。
    pub fn set_ripple(&mut self, x: f32, z: f32) -> bool {
        if !x.is_finite() || !z.is_finite() {
            return false;
        }
        let px = (((x.clamp(-256.0, 256.0) + 256.0) / 512.0) * 65535.0) as u32 & 0xffff;
        let pz = (((z.clamp(-256.0, 256.0) + 256.0) / 512.0) * 65535.0) as u32 & 0xffff;
        let slot = self.ripple_next % 3;
        self.ripple_packed[slot] = f32::from_bits((px << 16) | pz);
        self.ripple_age[slot] = 0.0;
        self.ripple_next = (slot + 1) % 3;
        true
    }

    fn next_ripples(&mut self) -> ([f32; 2], [f32; 4]) {
        for age in &mut self.ripple_age {
            if *age < 1.5 {
                *age += 1.0 / 60.0;
            }
        }
        (
            [self.ripple_packed[0], self.ripple_age[0]],
            [
                self.ripple_packed[1],
                self.ripple_age[1],
                self.ripple_packed[2],
                self.ripple_age[2],
            ],
        )
    }

    /// P3-R140：设置暗角/颗粒强度（各 0..=1）。
    pub fn set_post_fx(&mut self, vignette: f32, grain: f32) -> bool {
        if !vignette.is_finite() || !grain.is_finite() {
            return false;
        }
        self.vignette = vignette.clamp(0.0, 1.0);
        self.grain = grain.clamp(0.0, 1.0);
        true
    }

    /// 当前暗角/颗粒。
    #[must_use]
    pub const fn post_fx(&self) -> [f32; 2] {
        [self.vignette, self.grain]
    }

    /// PBR 变体是否已经编译（诊断/验收用）。
    #[must_use]
    pub fn pbr_pipelines_ready(&self) -> bool {
        self.pbr_pipelines_ready
    }

    /// 当前生效的渲染开关（诊断/验收用）。
    #[must_use]
    pub fn render_settings(&self) -> RenderSettings {
        self.render_settings
    }

    /// P3-R245：帧路径 GPU 位姿档（默认关）。关掉时暂存的批次不被消费，宿主侧提取
    /// 仍按 CPU `matrices_at` 填 `instances`，画面与开档前逐字节相同。
    ///
    /// 开档后 `frame_3d_draws` 对**本帧暂存的批次**放弃逐实例剔除与合批，改为
    /// "compute 烘焙进实例 arena + 单独一条 draw"；CPU 矩阵保留在 `FrameDraw.models`
    /// 里作回退（烘焙失败/无 compute 能力时照旧画）。
    pub fn set_gpu_motion_batches(&mut self, enabled: bool) {
        self.gpu_motion_batches = enabled;
    }

    /// P3-R245 开档状态（诊断/验收用）。
    #[must_use]
    pub fn gpu_motion_batches(&self) -> bool {
        self.gpu_motion_batches
    }

    /// P3-R250：GPU 位姿烘焙的逐实例距离剔除半径（米）。`0` = 关（默认，逐矩阵不变）。
    ///
    /// 只对走 GPU 烘焙的批次生效；半径 <= 0 或非有限值一律归 0。剔除球心是当帧相机
    /// 世界位置，判在实例**世界位置**上（不做包围球膨胀）⇒ 贴近切面的实例可能整只
    /// 消失，这是一条观感取舍档，不进 `--quality`。
    pub fn set_gpu_motion_cull_distance(&mut self, meters: f32) {
        self.gpu_motion_cull_distance = if meters.is_finite() && meters > 0.0 {
            meters
        } else {
            0.0
        };
    }

    /// P3-R250 剔除半径（诊断/验收用）。
    #[must_use]
    pub fn gpu_motion_cull_distance(&self) -> f32 {
        self.gpu_motion_cull_distance
    }

    /// P3-R255：逐实例视锥剔除档。`false` = 关（默认，逐矩阵不变）。
    ///
    /// 开档时主视图看不见的实例连槽位都不领 ⇒ 它们在阴影/planar 里也一起消失（矩阵共享），
    /// 这是与距离剔除同类的观感取舍；保守包围球只会少剔、不会误剔。
    pub fn set_gpu_motion_frustum_cull(&mut self, enabled: bool) {
        self.gpu_motion_frustum_cull = enabled;
    }

    /// P3-R255 视锥剔除档（诊断/验收用）。
    #[must_use]
    pub fn gpu_motion_frustum_cull(&self) -> bool {
        self.gpu_motion_frustum_cull
    }

    /// P3-R266：GPU 位姿烘焙的远程密度档：`(半径米, 步长)`。`None` / 步长 < 2 / 半径 <= 0
    /// 一律关（默认，逐矩阵不变）。开档后离相机超过半径的实例只留批次内
    /// `index % 步长 == 0` 的那些；矩阵共享 ⇒ 主视图/阴影/planar 一起变稀。
    pub fn set_gpu_motion_thin(&mut self, thin: Option<(f32, u32)>) {
        self.gpu_motion_thin =
            thin.filter(|(meters, stride)| meters.is_finite() && *meters > 0.0 && *stride >= 2);
    }

    /// P3-R266 远程密度档（诊断/验收用）。
    #[must_use]
    pub fn gpu_motion_thin(&self) -> Option<(f32, u32)> {
        self.gpu_motion_thin
    }

    /// P3-R267：第二远带 `(半径米, 步长)`——只在第一带已开时生效，且半径必须更大；
    /// 与第一带取**交集**（第一带已留下标集合，第二带再抽）。`None` = 关（默认）。
    pub fn set_gpu_motion_thin_far(&mut self, thin_far: Option<(f32, u32)>) {
        self.gpu_motion_thin_far =
            thin_far.filter(|(meters, stride)| meters.is_finite() && *meters > 0.0 && *stride >= 2);
    }

    /// P3-R267 第二远带（诊断/验收用）。
    #[must_use]
    pub fn gpu_motion_thin_far(&self) -> Option<(f32, u32)> {
        self.gpu_motion_thin_far
    }

    /// P3-R255：当帧烘焙用的主视图视锥（6 × `vec4` 打包）。`None` = 本帧没有主视图。
    #[must_use]
    pub fn bake_frustum(&self) -> Option<[[f32; 4]; 6]> {
        self.bake_frustum
    }

    /// P3-R245：为这一帧的 3D 提取登记一批运动实例。宿主在 `render` / `render_into_texture`
    /// **之前**调用；批次按 `entity`（`Scene3dDrawable::entity` 的 id）认领，一帧内没被
    /// 认领的在帧末丢弃，不会跨帧残留。`time` 是求值用的场景时钟（秒）。
    ///
    /// 只登记不生效：还要 [`Self::set_gpu_motion_batches`] 开档。同一实体重复登记以后者为准。
    pub fn stage_motion_batch(&mut self, entity: u64, batch: InstanceBatch3D, time: f32) {
        if let Some(existing) = self
            .staged_motion
            .iter_mut()
            .find(|(staged, _, _)| *staged == entity)
        {
            existing.1 = Arc::new(batch);
            existing.2 = time;
        } else {
            self.staged_motion.push((entity, Arc::new(batch), time));
        }
    }

    /// P3-R03-3：设置完整环境（`.rlenv` 的预过滤 cubemap + SH9 + 强度/旋转）。
    ///
    /// 上传失败返回原因，**不改变**当前环境——调用方可以退到 SH-only 并报警，而不是把
    /// 半个环境留在渲染器里。设置成功后 `ibl` 策略为 `Auto`/`On` 会解析到 `Full`
    /// （设备能力不足时由 [`Self::ibl_mode`] 降为 `Diffuse`）。
    pub fn set_environment_asset(
        &mut self,
        asset: &EnvironmentAsset,
        settings: EnvironmentSettings,
    ) -> Result<(), EnvironmentMapError> {
        let (texture, view) = environment::upload(&self.device, &self.queue, asset)?;
        self.environment_view = view;
        self.environment_map = Some(environment::GpuEnvironmentMap {
            _texture: texture,
            face_size: asset.face_size,
            mip_count: asset.mip_count(),
        });
        self.environment = Some(EnvironmentLighting {
            sh: asset.sh,
            settings,
        });
        self.rebuild_environment_bind_group();
        Ok(())
    }

    /// P3-R03-2：只设置 SH9 环境光照（无 cubemap）：`ibl` 落在 `Diffuse`，是 Tier C 的回退档。
    pub fn set_environment(&mut self, lighting: EnvironmentLighting) {
        self.environment = Some(lighting);
    }

    /// 清除环境（SH 与 cubemap 一起卸载），回到 R02 的常数环境项。
    pub fn clear_environment(&mut self) {
        self.environment = None;
        if self.environment_map.take().is_some() {
            self.environment_view = self.environment_fallback_view.clone();
            self.rebuild_environment_bind_group();
        }
    }

    /// 当前环境光照（诊断/测试用）。
    #[must_use]
    pub fn environment(&self) -> Option<EnvironmentLighting> {
        self.environment
    }

    /// 已上传环境 cubemap 的 `(face_size, mip_count)`；`None` = 没有镜面环境。
    #[must_use]
    pub fn environment_map_info(&self) -> Option<(u32, u32)> {
        self.environment_map
            .as_ref()
            .map(|map| (map.face_size, map.mip_count))
    }

    /// P3-R03-4：环境采样的设备档位（A/B/C），编辑器与日志据此显示质量与回退原因。
    #[must_use]
    pub fn ibl_tier(&self) -> IblTier {
        ibl_tier(
            self.capabilities.hdr_color,
            self.capabilities.float_linear_filter,
        )
    }

    /// Tier C 时镜面 IBL 不可用的**可读原因**（A/B 为 `None`）。
    #[must_use]
    pub fn ibl_specular_unavailable_reason(&self) -> Option<String> {
        let tier = self.ibl_tier();
        if tier.supports_specular() {
            return None;
        }
        Some(format!(
            "Tier {}：Rgba16Float cubemap 不可用（backend={}），镜面 IBL 关闭，仅 SH9 漫反射",
            tier.as_str(),
            self.capabilities.backend_name()
        ))
    }

    /// 本帧实际使用的 IBL 档位：策略 × 有没有环境 × 设备镜面能力。
    ///
    /// P3-R03-3 起镜面能力 = 已上传 cubemap 且设备能绑定/采样 `Rgba16Float`；
    /// 采样器是否线性由构造期能力决定（Nearest 走 Tier B），不影响档位解析。
    #[must_use]
    pub fn ibl_mode(&self) -> IblMode {
        if !self.render_settings.pbr {
            return IblMode::Off;
        }
        let specular_available = self.environment_map.is_some() && self.capabilities.hdr_color;
        resolve_ibl(
            self.render_settings,
            self.environment.is_some(),
            specular_available,
        )
    }

    fn rebuild_environment_bind_group(&mut self) {
        self.environment_bind_group = environment::bind_group(
            &self.device,
            &self.environment_bind_group_layout,
            &self.environment_buffer,
            &self.environment_view,
            &self.environment_sampler,
        );
    }

    /// 把本帧的环境 uniform 写进缓冲（160 B，每帧一次）。IBL 关闭时写全零，
    /// 着色器的 Off 分支根本不读它——写零是为了让"上一帧的环境"不会残留。
    /// `params.w` = 预过滤 mip 的最大 LOD（`mip_count - 1`），着色器用它映射 roughness。
    fn write_environment_uniform(&self, mode: IblMode) {
        let mut uniform = match self.environment {
            Some(lighting) => lighting.uniform(mode),
            None => EnvironmentLighting::default().uniform(IblMode::Off),
        };
        uniform[39] = self
            .environment_map
            .as_ref()
            .map_or(0.0, |map| map.max_lod());
        self.queue
            .write_buffer(&self.environment_buffer, 0, bytemuck::cast_slice(&uniform));
    }

    /// P3-R04-3：3D 网格/阴影管线变体缓存的命中/未命中/编译耗时/变体数。
    #[must_use]
    pub fn pipeline_cache_stats(&self) -> PipelineCacheStats {
        self.pipelines.stats()
    }

    /// 最近一次 HDR 场景颜色目标的尺寸；`(0, 0)` = 本进程还没创建过 HDR 目标。
    #[must_use]
    pub fn hdr_scene_size(&self) -> (u32, u32) {
        self.hdr_scene_size
    }

    /// 最近一帧的 HDR 场景颜色是否启用（与 [`Self::last_frame_metrics`] 同源）。
    #[must_use]
    pub fn hdr_active(&self) -> bool {
        self.last_frame_metrics.hdr_active
    }

    /// 最近一次成功提交的帧统计。GPU timestamp 不可用或尚未完成时为 `None`。
    pub fn last_frame_metrics(&self) -> RenderFrameMetrics {
        self.last_frame_metrics
    }

    /// P3-R200：点光 cube 阴影面的实际边长（构建期建出的纹理尺寸）。
    pub fn point_shadow_resolution(&self) -> u32 {
        self.point_shadow_resolution
    }

    /// P3-R202：当前点光阴影盏数预算（1..=8）。
    pub fn point_shadow_light_budget(&self) -> u32 {
        self.point_shadow_light_budget
    }

    /// P3-R202：运行期改盏数预算；clamp 到 1..=8，下一帧生效（不重建纹理/绑定）。
    pub fn set_point_shadow_light_budget(&mut self, budget: u32) {
        self.point_shadow_light_budget = budget.clamp(1, POINT_SHADOW_LIGHTS as u32);
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

    /// E06-3：上一帧的逐通道 GPU 时间。
    ///
    /// 设备不支持 timestamp 查询时返回 `supported=false` + 原因；支持但还没完成第一次
    /// 异步回读时返回 `supported=true`、`frame_us=None`。两者都不伪造 0。
    #[must_use]
    pub fn gpu_pass_report(&self) -> GpuPassReport {
        match self.timestamp_state {
            GpuTimestampState::Unsupported => GpuPassReport {
                supported: false,
                reason: Some("unsupported".to_owned()),
                ..GpuPassReport::default()
            },
            GpuTimestampState::MapError => GpuPassReport {
                supported: false,
                reason: Some(
                    self.timestamp_error
                        .clone()
                        .unwrap_or_else(|| "map_error".to_owned()),
                ),
                ..GpuPassReport::default()
            },
            GpuTimestampState::Ready => self.last_gpu_pass_report.clone(),
        }
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

    /// P3-R206 读数补全：本帧被最大绘制距离档剔掉的实例数（已含在 `frame_cull_frustum` 里）。
    #[must_use]
    pub fn frame_cull_distance(&self) -> u64 {
        self.frame_cull_distance
    }

    /// P3-R252：本帧因整批落在距离剔除球外而没登记 draw 的 GPU 位姿批次 draw 数。
    #[must_use]
    pub fn frame_gpu_motion_culled(&self) -> u64 {
        self.frame_gpu_motion_culled
    }

    /// P3-R254：本帧被整批跳过带走的实例数（批次内实例总和）。只在
    /// `gpu_motion_cull_distance > 0` 时非零。
    #[must_use]
    pub fn frame_gpu_motion_culled_instances(&self) -> u64 {
        self.frame_gpu_motion_culled_instances
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
        let variant_stats = self.pipelines.stats();
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
            pipeline_variants: variant_stats.variants,
            pipeline_variant_hits: variant_stats.hits,
            pipeline_variant_misses: variant_stats.misses,
            pipeline_compile_ms: variant_stats.compile_ms(),
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
#[cfg(test)]
fn light_view_projection(light: Light) -> Mat4 {
    light_view_projection_focused(light, Vec3::ZERO, 10.0, DEFAULT_SHADOW_RESOLUTION)
}

/// 两级阴影的半宽随相机距离变化：近级保住细节，远级覆盖大范围。
///
/// 上帝视角 270 m 时约 40 / 140 m 半宽（80 / 280 m 覆盖），比单级 60 m 多出
/// 一倍多的有效阴影范围；近景仍收在 20 / 60 m，不会为了远处牺牲脚下。
fn shadow_half_extents_for(distance: f32) -> [f32; 4] {
    let near = (distance * 0.16).clamp(20.0, 40.0);
    let mid = (distance * 0.34).clamp(45.0, 85.0);
    let far = (distance * 0.58).clamp(85.0, 150.0);
    // 第四级只兜住远端：半宽 160–260 m，270 m 视角下覆盖约 486 m 半径，
    // 补上三级 CSM 在 300 m 外的无阴影区；前三级保持原值，近景读数和既有 golden 不变。
    let ultra = (distance * 0.90).clamp(160.0, 260.0);
    // P3-R17 级联缩放量化：连续缩放的半宽会让纹素尺寸每帧变一点，焦点 snap 只能对齐
    // 平移、对不上缩放。把 2 m 作为一档向上取整，相机距离轻微变化时四级尺寸保持不变；
    // 上限仍夹在 40/85/150/260，不改变覆盖语义。
    let quantize = |value: f32, min: f32, max: f32| {
        if value <= min {
            min
        } else if value >= max {
            max
        } else {
            (value / 2.0).ceil() * 2.0
        }
    };
    [
        quantize(near, 20.0, 40.0),
        quantize(mid, 45.0, 85.0),
        quantize(far, 85.0, 150.0),
        quantize(ultra, 160.0, 260.0),
    ]
}

/// 点光阴影远平面：跟随灯的范围，至少 5 m、最多 100 m，避免远处小物体也进 6 个面。
/// P3-R202：盏数预算内挑出最近的 N 盏点光（相机焦点为参照），返回场景下标。
/// 排序键 =「距离平方升序、同距按下标」⇒ 槽位分配每帧稳定；着色器按灯位匹配槽位，
/// 所以重排槽位不改变采样正确性。
pub(super) fn point_shadow_light_indices(scene: &Scene3d, budget: usize) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..scene.point_lights.len().min(POINT_SHADOW_LIGHTS)).collect();
    indices.sort_by(|&a, &b| {
        let da = (scene.point_lights[a].position - scene.camera.target).length_squared();
        let db = (scene.point_lights[b].position - scene.camera.target).length_squared();
        da.total_cmp(&db).then(a.cmp(&b))
    });
    indices.truncate(budget.clamp(1, POINT_SHADOW_LIGHTS));
    indices
}

fn point_shadow_far(range: f32) -> f32 {
    (range.max(5.0) * 1.2).clamp(5.0, 100.0)
}

/// 6 个立方体面的透视视图投影（+X、-X、+Y、-Y、+Z、-Z）。
fn point_shadow_view_projections(position: Vec3, far: f32) -> [Mat4; 6] {
    let viewport = Viewport::new(POINT_SHADOW_SIZE as f32, POINT_SHADOW_SIZE as f32)
        .expect("point-shadow viewport is non-zero");
    let directions = [
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(-1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(0.0, 0.0, -1.0),
    ];
    let ups = [
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(0.0, 0.0, -1.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    ];
    let mut result = [Mat4::identity(); 6];
    for face in 0..6 {
        // `Camera3D::perspective` 固定世界 +Y 为上；±Y 面需要用 Z 作不共线参考，
        // 因此直接构造字段，让 `view_projection_matrix` 在最终 up 上做校验。
        let camera = Camera3D {
            position,
            target: position + directions[face],
            up: ups[face],
            fov_y: std::f32::consts::FRAC_PI_2,
            near: POINT_SHADOW_NEAR,
            far,
            ..Camera3D::default()
        };
        result[face] = camera
            .view_projection_matrix(viewport)
            .expect("constant point-shadow projection");
    }
    result
}

fn shadow_view_projections(light: Light, focus: Vec3, distance: f32, resolution: u32) -> [Mat4; 4] {
    let [near, mid, far, ultra] = shadow_half_extents_for(distance);
    [
        light_view_projection_focused(light, focus, near, resolution),
        light_view_projection_focused(light, focus, mid, resolution),
        light_view_projection_focused(light, focus, far, resolution),
        light_view_projection_focused(light, focus, ultra, resolution),
    ]
}

/// 跟随相机焦点的方向光阴影：把 2048² 阴影图铺在以 `focus` 为中心的正方形上。
///
/// 旧的固定原点 ±10 m 只够盖住出生点；上帝视角一拉远/平移，队列就走出阴影图，
/// 这是“看起来有阴影、实际上远处没有”的根因。这里用相机 `target` 做焦点，
/// 半宽 30 m（60 m 覆盖），并把焦点**在光源平面内**按纹素对齐；斜射光下只 snap
/// 世界 x/z 仍会抖，因此在 `right/up` 基上 snap，再重建焦点。
fn light_view_projection_focused(
    light: Light,
    focus: Vec3,
    half_extent: f32,
    resolution: u32,
) -> Mat4 {
    let direction = light
        .direction
        .normalized()
        .unwrap_or(Vec3::new(0.0, -1.0, 0.0));
    let half = half_extent.max(1.0);
    let texel = 2.0 * half / resolution.max(1) as f32;
    let snap = |value: f32| (value / texel).round() * texel;
    // 光源平面的正交基：`right × up` 与光方向一致时，相机姿态可复现。
    let up_hint = if direction.y.abs() > 0.99 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let right = direction
        .cross(up_hint)
        .normalized()
        .unwrap_or(Vec3::new(1.0, 0.0, 0.0));
    let up = right
        .cross(direction)
        .normalized()
        .unwrap_or(Vec3::new(0.0, 1.0, 0.0));
    let right_coord = focus.dot(right);
    let up_coord = focus.dot(up);
    let forward_coord = focus.dot(direction);
    // 三轴都在光源空间对齐：只对齐 right/up 时，焦点沿光方向的分量仍会改矩阵平移，
    // 斜射光下依旧逐帧抖。
    let focus = direction * snap(forward_coord) + right * snap(right_coord) + up * snap(up_coord);
    // 阴影相机**只消费姿态**（`view_matrix`）：它自带一份正交式投影（见下），透视投影的
    // 宽高比在这里从来不参与，因此拆分批 7 之后这台相机也不需要任何视口。
    let camera = Camera3D {
        position: focus - direction * 12.0,
        target: focus,
        up: up_hint,
        fov_y: 1.3,
        near: 0.1,
        far: 30.0,
        ..Camera3D::default()
    };
    let Ok(view) = camera.view_matrix() else {
        return Mat4::identity();
    };
    let scale = 1.0 / half;
    let projection = Mat4 {
        columns: [
            [scale, 0.0, 0.0, 0.0],
            [0.0, scale, 0.0, 0.0],
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
