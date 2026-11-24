//! `Renderer` 的构造侧（WP12 阶段 4d 从 `renderer/mod.rs` 原样搬出）：`new`、
//! `new_offscreen` 与 `new_async`——适配器/设备/表面、各条管线、采样器与
//! 各张 GPU 缓冲的创建都在这里。
//!
//! ## `new_async` 的分批拆分（2026-11-18 完成）
//!
//! WP12 当时定的是"**只搬位置，不拆函数体**"，拆函数体留给"PBR/HDR 落地时顺势做"
//! 的工作包。PBR/HDR 已于 2026-10-23/24 交付，而那个包没开——`new_async` 从搬迁时的
//! 1,070 行涨到 **2,371 行（占全文件 70%）**。`docs/62` 的 1.7 版更正登记了这笔账。
//!
//! ### 结果
//!
//! | | 拆分前 | 拆分后 |
//! |---|---|---|
//! | `new_async` | 2,371 行 | **646 行（减 73%）** |
//! | `construct/` 子模块 | 1（`device`） | **18** |
//!
//! 剩下的 646 行是**编排**：函数签名、17 个子模块的调用与变量绑定、
//! `Ok(Self { ... })` 结构体字面量（约 190 行），以及少量"建完之后的一次性初始化"
//! （阴影清屏、深度视图、时间戳查询）。**已无可独立搬走的资源创建块。**
//!
//! ### 等价判据
//!
//! 每批的等价判据是 `tests/construct_creation_order.rs` 里的**资源创建序列**：
//! 92 次 `device.create_*` 的类型与顺序一个都不能变。（文件级拆分用的"纯搬运"证明
//! 对拆函数体不成立——变量要重新绑定、作用域要重新组织，行段必然变。）
//!
//! ### 批次表
//!
//! 拆到最后实际做了 17 批，编号**不按行号**（原因见下）。按依赖层级排列：
//!
//! | 批次 | 内容 | 模块 | 规模 |
//! |---|---|---|---|
//! | 1 | 设备与能力探测 | [`device::boot_device`] | 148 行 / 0 处 |
//! | A | 相机绑定组布局 | [`base_layouts`] | 101 行 / 1 处 |
//! | 2 | 逐纹理采样器表 | [`texture`] | 51 行 / 1 处 |
//! | B | 文本 / 矢量字管线 | [`text_pipeline`] | 112 行 / 10 处 |
//! | C | 材质 3D 绑定布局 | [`material_layout`] | 110 行 / 1 处 |
//! | D | 相机、光照与初始 uniform | [`camera_scene`] | 114 行 / 10 处 |
//! | E | 帧环与 albedo 采样器 | [`frame_arena`] | 24 行 / 3 处 |
//! | 4 | 白图 / 阴影纹理 | [`shadow_textures`] | 115 行 / 5 处 |
//! | G | ESM 方向光阴影 | [`esm`] | 84 行 / 6 处 |
//! | I | 阴影绑定组 | [`shadow_bindings`] | 35 行 / 3 处 |
//! | J | 环境光照 group | [`environment_group`] | 21 行 / 1 处 |
//! | H | 相机绑定与平面反射 | [`camera_bindings`] | 142 行 / 4 处 |
//! | K | 三条管线布局 | [`pipeline_layouts`] | 22 行 / 3 处 |
//! | L | 变体预编译 | [`variant_compile`] | 83 行 / 0 处（在闭包里） |
//! | 7 | 世界 Sprite | [`world_sprite`] | 86 行 / 6 处 |
//! | 8 | GPU 粒子布局 | [`gpu_particles`] | 67 行 / 3 处 |
//! | 9 | 后处理（Tonemap/SSAO/SSR） | [`post_process`] | 661 行 / 31 处 |
//!
//! ### 三条方法论（都是踩出来的，不是想出来的）
//!
//! **① 按依赖关系切，不按行号切。** 批次 1 刻意选**唯一不含资源创建**的段落——
//! 它是验证拆分手法本身的最低风险样板。批次 2 则验证了：`texture_samplers` 自包含
//! （只吃 `device` 加两个静态方法），不前依赖后文、后文也不改它，所以能单独搬 51 行。
//! **批次划分的价值在于每批都能独立验证，不在于每批够大。**
//!
//! **② "物理位置相邻"和"依赖相邻"是两件事。** 一度以为 `camera_bind_group_layout`
//! 是挡路的结构障碍（它物理上夹在别的段落之间）。精密测过才发现它只吃 `device`、
//! 零其他输入、产出恰好一个布局——它其实是整张依赖图的**根**。按批次 A 拆掉后，
//! 一批原本以为要绕开的东西一下子全通了。
//!
//! **③ 探查出的"依赖"要读代码确认，grep 到变量名不等于依赖它的值。** 至少三次踩到：
//! 批次 D 以为要 B 的 `sampler`（实际阴影采样器自己现建）、批次 G 以为要 B 的
//! `shader`/`pipeline`（实际 ESM 用独立的 `crate::shader::ESM`）、批次 H 以为要
//! `format`/`camera`（实际没用到）。编译器每次都兜住了，但那是运气——**该读的时候要读**。
//!
//! ### 子模块写作约定
//!
//! 每个子模块的文档头统一写四节：**这一段原本在哪**、**依赖面**（列全，标注来自哪一批）、
//! **语义零变化的等价点**、以及**为什么这么切**（当切法不显然时）。
//! 这样下一个动这块的人不用重新推导一遍依赖。
//!
//! ## 关于 `Renderer` 结构体
//!
//! `Renderer` 结构体与全部字段仍留在 `renderer/mod.rs`（§3.1.2）；本模块是它的后代，
//! 结构体字面量按私有字段构造**不需要**放宽任何可见性——子模块用 `pub(super)` 取用
//! 父模块的私有项。

use crate::{
    shader::{MESH3D, MESH3D_PBR, SHADOW, ShaderSource},
    shader_preprocess::{PreprocessError, PreprocessOptions, PreprocessedShader, preprocess},
};
use rustlua_camera::camera::FrameViews;
use rustlua_math::math3d::{Mat4, Vec3};
use rustlua_render::async_loader::{AsyncLoader, LoadMode};
use rustlua_render::frame::{OverlayVertex, VectorTextVertex, WorldSpriteVertex};
use rustlua_render::render_plan::RenderPlan;
use rustlua_render::render_settings::{IblMode, RenderSettings};
use std::{borrow::Cow, collections::HashMap, path::PathBuf, sync::Arc};
use wgpu::{Device, Queue};
use winit::window::Window;

use super::pipeline_cache::{
    ColorTarget, InstanceMode, PipelineFamily, ShaderVariantKey, Shading, SkinMode,
    VARIANT_COMPILE_BUDGET_MS,
};
use super::world_sprite::WORLD_SPRITE_BUFFER_BYTES;
pub(super) mod base_layouts;
pub(super) mod camera_bindings;
pub(super) mod camera_scene;
pub(super) mod device;
pub(super) mod environment_group;
pub(super) mod esm;
pub(super) mod frame_arena;
pub(super) mod gpu_particles;
pub(super) mod material_layout;
pub(super) mod pipeline_layouts;
pub(super) mod post_process;
pub(super) mod shadow_bindings;
pub(super) mod shadow_textures;
pub(super) mod text_pipeline;
pub(super) mod texture;
pub(super) mod variant_compile;
pub(super) mod world_sprite;

use super::{
    DepthPass, FrameDraw, GpuPassReport, GpuTimestampState, RenderFrameMetrics, Renderer,
    compute::ComputeState, depth_stencil_state,
};

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
    ) -> anyhow::Result<Self> {
        Self::new_with_shadow_resolution(
            window,
            project_root,
            cull_enabled,
            instancing_enabled,
            super::DEFAULT_SHADOW_RESOLUTION,
            super::DEFAULT_SHADOW_CASCADE_LIMIT,
            super::DEFAULT_POINT_SHADOW_RESOLUTION,
        )
    }

    /// P3-R198/R199/R200：带阴影品质的选择入口。方向光纹理与绑定组按 `shadow_resolution`
    /// 一次建好（512..=4096）；`shadow_cascade_limit`（1..=4）决定实际渲染的级联层数，
    /// 未渲染的远级联清成全亮；`point_shadow_resolution`（512/1024）决定点光 cube 面的边长。
    /// 缺省值 = 旧行为。
    pub fn new_with_shadow_resolution(
        window: Arc<Window>,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
        shadow_resolution: u32,
        shadow_cascade_limit: u32,
        point_shadow_resolution: u32,
    ) -> anyhow::Result<Self> {
        pollster::block_on(Self::new_async(
            Some(window),
            None,
            project_root,
            cull_enabled,
            instancing_enabled,
            shadow_resolution.clamp(512, 4096),
            shadow_cascade_limit.clamp(1, 4),
            point_shadow_resolution.clamp(512, 1024),
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
            super::DEFAULT_SHADOW_RESOLUTION,
            super::DEFAULT_SHADOW_CASCADE_LIMIT,
            super::DEFAULT_POINT_SHADOW_RESOLUTION,
        ))
    }

    /// P3-R198/R199/R200：离屏渲染的阴影品质入口（测试/编辑器预览用）。
    #[allow(clippy::too_many_arguments)] // 构造器一次性把三档阴影参数透传到底，拆结构体只会加一层映射。
    pub fn new_offscreen_with_shadow_settings(
        device: Device,
        queue: Queue,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
        shadow_resolution: u32,
        shadow_cascade_limit: u32,
        point_shadow_resolution: u32,
    ) -> anyhow::Result<Self> {
        pollster::block_on(Self::new_async(
            None,
            Some((device, queue)),
            project_root,
            cull_enabled,
            instancing_enabled,
            shadow_resolution.clamp(512, 4096),
            shadow_cascade_limit.clamp(1, 4),
            point_shadow_resolution.clamp(512, 1024),
        ))
    }

    /// [`Self::new_offscreen`] 的**异步**版本，供没有阻塞能力的宿主使用（web）。
    ///
    /// 为什么需要另开一个入口：`new_offscreen` 用 `pollster::block_on` 驱动，而
    /// `wasm32-unknown-unknown` 上 `thread::park_timeout` 未实现——一旦 `new_async` 里
    /// 任何一个 await 点没有在首次 poll 就绪，它就会 panic。浏览器里正确的形态是**由调用方
    /// await**（wasm-bindgen 的 `async fn` 导出会变成 Promise，正好接上 JS 微任务）。
    /// 原生侧行为一字不变：`new_offscreen` 仍是同一段 `new_async`，只是这里把等待方式交给调用方。
    pub async fn new_offscreen_async(
        device: Device,
        queue: Queue,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
    ) -> anyhow::Result<Self> {
        Self::new_async(
            None,
            Some((device, queue)),
            project_root,
            cull_enabled,
            instancing_enabled,
            super::DEFAULT_SHADOW_RESOLUTION,
            super::DEFAULT_SHADOW_CASCADE_LIMIT,
            super::DEFAULT_POINT_SHADOW_RESOLUTION,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)] // 同上：窗口/离屏共用的唯一构造内核。
    async fn new_async(
        window: Option<Arc<Window>>,
        external_device: Option<(Device, Queue)>,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
        shadow_resolution: u32,
        shadow_cascade_limit: u32,
        point_shadow_resolution: u32,
    ) -> anyhow::Result<Self> {
        // P0-②：窗口渲染（游戏运行）走异步加载；离屏渲染（golden / 编辑器预览）走同步，
        // 保证确定性输出里绝无异步加载。
        let async_mode = if window.is_some() {
            LoadMode::Async
        } else {
            LoadMode::Sync
        };
        // 设备、能力快照与派生开关：拆分批 1 搬到 `construct/device.rs`。
        // 这一段一处资源创建都没有，所以它是验证拆分手法的最低风险样板。
        let boot =
            super::construct::device::boot_device(window, external_device, instancing_enabled)
                .await?;
        let device = boot.device;
        let queue = boot.queue;
        let surface = boot.surface;
        let format = boot.format;
        let config = boot.config;
        let adapter_name = boot.adapter_name;
        let storage_capable = boot.storage_capable;
        let storage_usage = boot.storage_usage;
        let instancing_enabled = boot.instancing_enabled;
        let capabilities = boot.capabilities;
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
        // 相机 3D 绑定组布局：拆分批 A 搬到 `construct/base_layouts.rs`。
        // 这一段只吃 device、零其他输入，是整张依赖图的根。
        let camera_bind_group_layout =
            super::construct::base_layouts::camera_bind_group_layout(&device);
        // 2D 贴图/文本/矢量字：拆分批 B 搬到 `construct/text_pipeline.rs`。
        // 它产出 sampler/shader/pipeline，被 D 相机光照、G ESM、J 环境三处依赖 ——
        // 拆了它 D 才能拆。
        let tp = super::construct::text_pipeline::text_pipeline(
            &device,
            format,
            storage_capable,
            storage_usage,
            &pipeline_layout,
        );
        let _shader = tp.shader;
        let pipeline = tp.pipeline;
        let sampler = tp.sampler;
        let texture_samplers = tp.texture_samplers;
        let sdf_sampler = tp.sdf_sampler;
        let pipeline_sdf = tp.pipeline_sdf;
        let vector_bind_group_layout = tp.vector_bind_group_layout;
        let vector_layout = tp.vector_layout;
        let _vector_shader = tp.vector_shader;
        let vector_pipeline = tp.vector_pipeline;
        let vector_vertex_buffer = tp.vector_vertex_buffer;
        let vector_glyph_buffer = tp.vector_glyph_buffer;
        let vector_segment_buffer = tp.vector_segment_buffer;
        let vertex_buffer = tp.vertex_buffer;
        // 材质布局里有 1 个是**存储缓冲**（`binding 2` = 逐实例模型矩阵，给实例化用）。
        // 能力不足时不放它：布局本身仍然合法（原生那条路完全不变），而依赖它的管线会走占位
        // （见下面的 `mesh_pipeline` / `shadow_pipeline`），3D 通道在 `passes.rs` 里也不再声明。
        // 材质 3D 绑定组布局：拆分批 C 搬到 `construct/material_layout.rs`。
        // 只吃 device + storage_capable，零其他依赖；三条管线布局（批次 K）都要用它。
        let material_bind_group_layout =
            super::construct::material_layout::material_bind_group_layout(&device, storage_capable);
        // 首帧 uniform 的初值：相机矩阵按**目标视口**算（宽高比只从视口来，拆分批 7）。
        // 相机/光照/初始 uniform：拆分批 D 搬到 `construct/camera_scene.rs`。
        // 依赖 A 的 camera layout 与 B 的 sampler —— 两个前置都已就位。
        let cs = super::construct::camera_scene::camera_scene(&device, &config, shadow_resolution)?;
        let _initial_viewport = cs.initial_viewport;
        let _camera = cs.camera;
        let _camera_matrix = cs.camera_matrix;
        let _light_camera = cs.light_camera;
        let _light_view = cs.light_view;
        let _light_projection = cs.light_projection;
        let _light_matrix = cs.light_matrix;
        let shadow_sampler = cs.shadow_sampler;
        let point_shadow_plain_sampler = cs.point_shadow_plain_sampler;
        let shadow_sample_view = cs.shadow_sample_view;
        let shadow_views = cs.shadow_views;
        let camera_buffer = cs.camera_buffer;
        let light_buffer = cs.light_buffer;
        let shadow_camera_buffer = cs.shadow_camera_buffer;
        let shadow_pass_camera_buffers = cs.shadow_pass_camera_buffers;
        let identity_palette = cs.identity_palette;
        // 帧环与地形采样器：拆分批 E 搬到 `construct/frame_arena.rs`。
        let fa = super::construct::frame_arena::frame_arena(&device, storage_usage);
        let uniform_align = fa.uniform_align;
        let storage_align = fa.storage_align;
        let material_arena = fa.material_arena;
        let instance_arena = fa.instance_arena;
        let albedo_sampler = fa.albedo_sampler;
        // 白贴图与阴影纹理：拆分批 4 搬到 `construct/shadow_textures.rs`。
        // 它不经过 camera_bind_group_layout，所以能先于批次 3 拆。
        // 阴影清屏（`if shadow_cascade_limit < 4`）留在下面：它消费这里的产物、
        // 还要读构造参数，本质是"建完之后的一次性初始化"，不是"建资源"。
        let shadows = super::construct::shadow_textures::shadow_textures(
            &device,
            &queue,
            format,
            point_shadow_resolution,
        );
        // 白贴图本体后文不直接持有（只用其视图），但结构体里保留以便热重载时重建。
        let _white_texture = shadows.white_texture;
        let white_albedo_view = shadows.white_albedo_view;
        let _point_shadow_texture = shadows.point_shadow_texture;
        let point_shadow_resolution = shadows.point_shadow_resolution;
        let point_shadow_array_view = shadows.point_shadow_array_view;
        let point_shadow_face_views = shadows.point_shadow_face_views;
        let point_shadow_uniform_buffer = shadows.point_shadow_uniform_buffer;
        let point_shadow_camera_buffers = shadows.point_shadow_camera_buffers;
        let _esm_texture = shadows.esm_texture;
        let esm_array_view = shadows.esm_array_view;
        let esm_layer_views = shadows.esm_layer_views;
        // P3-R199：未渲染的远级联在构建期一次性清成「全亮」——深度 1.0 / ESM 白。
        // 采样这些层的像素只会失去该距离的阴影，不会被上一帧或未初始化内容误压暗。
        if shadow_cascade_limit < 4 {
            let mut clear_encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("rustlua-shadow-cascade-clear"),
                });
            for cascade in shadow_cascade_limit as usize..4 {
                let _ = clear_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("rustlua-shadow-cascade-depth-clear"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &shadow_views[cascade],
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                let _ = clear_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("rustlua-esm-cascade-clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &esm_layer_views[cascade],
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            }
            queue.submit(Some(clear_encoder.finish()));
        }
        // ESM 方向光阴影：拆分批 G 搬到 `construct/esm.rs`。
        // ESM 存 exp(k·z) 而非深度，可线性过滤 → 远处软边比 CSM 平滑。
        let esm = super::construct::esm::esm_shadow(&device, &shadow_sample_view);
        let _esm_bind_group_layout = esm.bind_group_layout;
        let _esm_layer_buffers = esm.layer_buffers;
        let esm_bind_groups = esm.bind_groups;
        let _esm_shader = esm.shader;
        let _esm_pipeline_layout = esm.pipeline_layout;
        let esm_pipeline = esm.pipeline;
        // 相机绑定组与平面反射：拆分批 H 搬到 `construct/camera_bindings.rs`。
        // 10 个输入全部来自 A/D/F，所以它是最后一个能拆的批次。
        let cb = super::construct::camera_bindings::camera_bindings(
            &device,
            &queue,
            &camera_bind_group_layout,
            &camera_buffer,
            &light_buffer,
            &shadow_camera_buffer,
            &point_shadow_uniform_buffer,
            &point_shadow_array_view,
            &point_shadow_plain_sampler,
            &shadow_sampler,
            &esm_array_view,
            &shadow_sample_view,
        );
        let camera_bind_group = cb.camera_bind_group;
        let planar_camera_buffer = cb.planar_camera_buffer;
        let planar_camera_bind_group = cb.planar_camera_bind_group;
        let planar_reflection_texture = cb.planar_reflection_texture;
        let planar_reflection_view = cb.planar_reflection_view;
        let planar_reflection_depth = cb.planar_reflection_depth;
        // 阴影绑定：拆分批 I 搬到 `construct/shadow_bindings.rs`。
        // 方向光级联与点光立方体面共用一个 64B uniform 的 layout。
        let sb = super::construct::shadow_bindings::shadow_bindings(
            &device,
            &shadow_pass_camera_buffers,
            &point_shadow_camera_buffers,
        );
        let shadow_bind_group_layout = sb.layout;
        let shadow_pass_bind_groups = sb.cascade_bind_groups;
        let point_shadow_bind_groups = sb.point_bind_groups;
        // 环境光照 group：拆分批 J 搬到 `construct/environment_group.rs`。
        let env =
            super::construct::environment_group::environment_group(&device, &queue, &capabilities);
        let environment_bind_group_layout = env.bind_group_layout;
        let environment_buffer = env.buffer;
        let environment_fallback_view = env.fallback_view;
        let environment_sampler = env.sampler;
        let environment_view = env.view;
        let environment_bind_group = env.bind_group;
        // 三条管线布局：拆分批 K 搬到 `construct/pipeline_layouts.rs`。
        // 四个输入布局分别来自 A / C / J / I，所以这一批只是拼装。
        let pl = super::construct::pipeline_layouts::pipeline_layouts(
            &device,
            &camera_bind_group_layout,
            &material_bind_group_layout,
            &environment_bind_group_layout,
            &shadow_bind_group_layout,
        );
        let mesh_pipeline_layout = pl.mesh;
        let pbr_pipeline_layout = pl.pbr;
        let shadow_pipeline_layout = pl.shadow;
        // P3-R04-3：按固定轴预编译变体。LDR 目标格式来自表面配置；HDR 变体只在能力允许时
        // 存在（与 R01 的门控一致）。实例矩阵的取法由设备能力决定（storage 数组 / 64 字节
        // uniform）——两条路都编译**真管线**：WebGL2 没有存储缓冲，但画的是同一批网格/阴影。
        // 变体预编译：拆分批 L 搬到 `construct/variant_compile.rs`。
        // 管线都在 PipelineCache 的闭包里建，所以这段没有直接的 device.create_*。
        let v = super::construct::variant_compile::compile_variants(
            &device,
            format,
            &capabilities,
            storage_capable,
            &mesh_pipeline_layout,
            &shadow_pipeline_layout,
        );
        let _instance_mode = v.instance_mode;
        let pipelines = v.pipelines;
        let _hdr_format = v.hdr_format;
        let _variant_stats = v.stats;
        let depth_view = Self::create_depth_view(&device, config.width, config.height);
        let depth_init = (config.width, config.height);
        let timestamp_enabled = device.features().contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        );
        let timestamp_query_set = timestamp_enabled.then(|| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("rustlua-frame-timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: super::pass_timestamps::TIMESTAMP_QUERY_COUNT,
            })
        });
        let timestamp_period_ns = queue.get_timestamp_period();
        // 世界 Sprite 通道：拆分批 7 搬到 `construct/world_sprite.rs`。
        // 相对独立（只吃 device/format/capabilities/texture_bind_group_layout），
        // 但不在按行号的批次计划里——它比批次 3 更早可拆，所以先做了。
        let ws = super::construct::world_sprite::world_sprite_resources(
            &device,
            format,
            &capabilities,
            &texture_bind_group_layout,
        );
        let world_sprite_camera_layout = ws.camera_layout;
        let world_sprite_pipeline_layout = ws.pipeline_layout;
        let world_sprite_camera_buffer = ws.camera_buffer;
        let world_sprite_camera_bind_group = ws.camera_bind_group;
        let world_sprite_pipeline = ws.pipeline;
        let world_sprite_opaque_pipeline = ws.opaque_pipeline;
        let world_sprite_additive_pipeline = ws.additive_pipeline;
        let hdr_world_sprite_pipeline = ws.hdr_pipeline;
        let hdr_world_sprite_opaque_pipeline = ws.hdr_opaque_pipeline;
        let hdr_world_sprite_additive_pipeline = ws.hdr_additive_pipeline;
        let world_sprite_buffer = ws.vertex_buffer;
        // GPU 粒子布局：拆分批 8 搬到 `construct/gpu_particles.rs`。
        // 它引用批次 7 的 world_sprite_camera_layout，所以只能在批次 7 之后做。
        let (gpu_particle_draw_layout, gpu_particle_pipeline_layout, gpu_particle_depth_layout) =
            super::construct::gpu_particles::gpu_particle_layouts(
                &device,
                storage_capable,
                &world_sprite_camera_layout,
                &texture_bind_group_layout,
            );
        let world_sprite_depth_view = Self::create_depth_view(&device, 1, 1);
        // 后处理通道：拆分批 9 搬到 `construct/post_process.rs`（661 行、31 处）。
        // 它排在 new_async 后半段，但只在下面 Ok(Self{..}) 里才碰到早期变量，
        // 创建资源本身只吃 device/queue/format/capabilities —— 所以能独立搬。
        let pp = super::construct::post_process::post_process_resources(
            &device,
            &queue,
            format,
            &capabilities,
        );
        let _post_process_shader = pp.post_process_shader;
        let post_process_layout = pp.post_process_layout;
        let post_process_pipeline_layout = pp.post_process_pipeline_layout;
        let post_process_pipeline = pp.post_process_pipeline;
        let post_process_sampler = pp.post_process_sampler;
        let post_process_params = pp.post_process_params;
        let tonemap_layout = pp.tonemap_layout;
        let _tonemap_pipeline_layout = pp.tonemap_pipeline_layout;
        let tonemap_pipeline = pp.tonemap_pipeline;
        let tonemap_params = pp.tonemap_params;
        let section_height_white = pp.section_height_white;
        let ssao_bind_group_layout = pp.ssao_bind_group_layout;
        let _ssao_pipeline_layout = pp.ssao_pipeline_layout;
        let _ssao_shader = pp.ssao_shader;
        let ssao_pipeline = pp.ssao_pipeline;
        let ssao_params = pp.ssao_params;
        let _ssao_white = pp.ssao_white;
        let ssao_raw_view = pp.ssao_raw_view;
        let _ssao_blur_white = pp.ssao_blur_white;
        let ssao_view = pp.ssao_view;
        let ssao_blur_bind_group_layout = pp.ssao_blur_bind_group_layout;
        let _ssao_blur_pipeline_layout = pp.ssao_blur_pipeline_layout;
        let _ssao_blur_shader = pp.ssao_blur_shader;
        let ssao_blur_pipeline = pp.ssao_blur_pipeline;
        let ssao_blur_params = pp.ssao_blur_params;
        let ssr_bind_group_layout = pp.ssr_bind_group_layout;
        let _ssr_pipeline_layout = pp.ssr_pipeline_layout;
        let _ssr_shader = pp.ssr_shader;
        let ssr_pipeline = pp.ssr_pipeline;
        let ssr_params = pp.ssr_params;
        let _ssr_white = pp.ssr_white;
        let ssr_raw_view = pp.ssr_raw_view;
        let ssr_blur_white = pp.ssr_blur_white;
        let ssr_view = pp.ssr_view;
        let ssr_history_white = pp.ssr_history_white;
        let ssr_history_view = pp.ssr_history_view;
        let ssr_blur_bind_group_layout = pp.ssr_blur_bind_group_layout;
        let _ssr_blur_pipeline_layout = pp.ssr_blur_pipeline_layout;
        let _ssr_blur_shader = pp.ssr_blur_shader;
        let ssr_blur_pipeline = pp.ssr_blur_pipeline;
        let ssr_blur_params = pp.ssr_blur_params;
        tracing::info!(target: "game",
            "[render] initialized: adapter={adapter_name} format={format:?} present_mode={:?} gpu_timestamp={timestamp_enabled}",
            config.present_mode
        );
        let ssao_depth_view = depth_view.clone();
        // P3：回读器在 `device`/`queue` 移交进结构体之前建好，内部各自持有一份克隆句柄。
        let readback_reader = crate::renderer::async_readback::AsyncReadback::new(&device, &queue);
        Ok(Self {
            surface,
            readback: readback_reader,
            device,
            queue,
            config,
            pipeline,
            pipelines,
            #[cfg(not(target_arch = "wasm32"))]
            mesh_pipeline_layout: mesh_pipeline_layout.clone(),
            pbr_pipeline_layout,
            shadow_pipeline_layout,
            #[cfg(not(target_arch = "wasm32"))]
            shader_library: crate::shader_library::ShaderLibrary::builtin(),
            #[cfg(not(target_arch = "wasm32"))]
            shader_reload: None,
            #[cfg(not(target_arch = "wasm32"))]
            shader_committed: HashMap::new(),
            #[cfg(not(target_arch = "wasm32"))]
            fixed_slot_layouts: crate::renderer::shader_hot_reload::FixedSlotLayouts {
                textured: pipeline_layout.clone(),
                vector_text: vector_layout.clone(),
                world_sprite: world_sprite_pipeline_layout.clone(),
                post_process: post_process_pipeline_layout.clone(),
            },
            pbr_pipelines_ready: false,
            environment_buffer,
            environment_bind_group,
            environment_bind_group_layout,
            environment_fallback_view,
            environment_sampler,
            environment_view,
            environment_map: None,
            environment: None,
            camera_bind_group,
            shadow_pass_bind_groups,
            shadow_pass_camera_buffers,
            point_shadow_face_views,
            point_shadow_uniform_buffer,
            point_shadow_camera_buffers,
            point_shadow_bind_groups,
            depth_view,
            depth_size: depth_init,
            shadow_views,
            esm_layer_views,
            esm_pipeline,
            esm_bind_groups,
            ssao_depth_view,
            texture_bind_group_layout,
            sampler,
            texture_samplers,
            resource_meta: None,
            resource_meta_loaded: false,
            sampling_epoch: 0,
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
            storage_capable,
            capabilities,
            compute_state: ComputeState::new(),
            gpu_particles: Vec::new(),
            gpu_particle_wind: [0.0, 0.0],
            gpu_particle_wind_z: 0.0,
            gpu_particle_wind_shear: 0.0,
            gpu_particle_wind_gust: 0.0,
            gpu_particle_wind_gust_hz: 0.0,
            gpu_particle_wind_sources: [[0.0; 7]; 4],
            gpu_particle_time: 0.0,
            gpu_particle_grid: None,
            cloud_coverage: 0.5,
            cloud_speed: 1.0,
            cloud_base: 100.0,
            cloud_shadow_strength: 0.26,
            flash_intensity: 0.0,
            water_tint: [1.0, 1.0, 1.0, 0.0],
            surface_cover: [0.0, 0.0],
            exposure_offset: 0.0,
            ripple_packed: [0.0; 3],
            ripple_age: [1.5; 3],
            ripple_next: 0,
            vignette: 0.0,
            grain: 0.0,
            gpu_particle_generation: 0,
            gpu_particle_draw_layout,
            gpu_particle_depth_layout,
            gpu_particle_pipeline_layout,
            particle_softness: rustlua_render::render_config::DEFAULT_PARTICLE_SOFTNESS,
            gpu_particle_pipelines: Vec::new(),
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
            frame_bind_build_us: 0,
            frame_bind_bake_us: 0,
            instance_scratch: Vec::new(),
            pending_compute: None,
            pending_compute_sealed: None,
            bake_inputs: std::collections::HashMap::new(),
            bake_input_allocs: 0,
            bake_static_uploads: 0,
            bake_bind_serials: 1u64 << 63,
            bake_cache_enabled: true,
            gpu_motion_batches: false,
            gpu_motion_cull_distance: 0.0,
            gpu_motion_frustum_cull: false,
            gpu_motion_thin: None,
            gpu_motion_thin_far: None,
            bake_frustum: None,
            camera_world_position: Vec3::ZERO,
            staged_motion: Vec::new(),
            material_bind_cache: Vec::new(),
            material_arena_epoch: 0,
            instance_arena_epoch: 0,
            material_bind_created: 0,
            material_bind_reused: 0,
            material_bind_frames: 0,
            material_bind_reported: false,
            uniform_align,
            storage_align,
            mesh_cache: HashMap::new(),
            mesh_slots: HashMap::new(),
            mesh_identity_hits: 0,
            mesh_identity_misses: 0,
            albedo_textures: Vec::new(),
            linear_textures: Vec::new(),
            texture_content_cache: HashMap::new(),
            material_cache: HashMap::new(),
            generated_textures: HashMap::new(),
            material_preview_path: None,
            material_preview_textures: None,
            material_preview_indices: None,
            albedo_sampler,
            white_albedo_view,
            cull_enabled,
            frame_cull_candidates: 0,
            frame_cull_frustum: 0,
            frame_cull_distance: 0,
            skip_mesh_filter: None,
            frame_skip_mesh: 0,
            frame_gpu_motion_culled: 0,
            frame_gpu_motion_culled_instances: 0,
            depth_sort_enabled: true,
            planar_cull_enabled: true,
            planar_skip_entities: Vec::new(),
            max_draw_distance: 0.0,
            max_draw_distance_filter: None,
            shadow_caster_skip: None,
            point_shadow_caster_skip: None,
            shadow_caster_max_distance: 0.0,
            water_refraction_gain: 0.0,
            submerged_view_gain: 0.0,
            water_refract_steps: 0,
            clip_cap_depth: 0.0,
            clip_cap_both_sides: false,
            point_face_cull_enabled: true,
            planar_reflection_divisor: 2,
            water_cheap_override: false,
            terrain_cheap_override: false,
            crowd_cheap_override: false,
            shadow_resolution,
            shadow_cascade_limit,
            point_shadow_resolution,
            point_shadow_light_budget: super::POINT_SHADOW_LIGHTS as u32,
            instancing_enabled,
            frame_count: 0,
            clock: web_time::Instant::now(),
            clock_frozen_secs: None,
            _material_bind_group_layout: material_bind_group_layout,
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
            last_gpu_pass_report: GpuPassReport::default(),
            last_frame_plan: RenderPlan::empty(),
            last_frame_passes: Vec::new(),
            frame_views: FrameViews::new(),
            world_sprite_pipeline,
            world_sprite_opaque_pipeline,
            world_sprite_additive_pipeline,
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
            // M84 P1：Spine 缓冲懒创建（首个非空 Spine 帧才分配设备资源）。
            spine_vertex_buffer: None,
            spine_vertex_capacity: 0,
            spine_index_buffer: None,
            spine_index_capacity: 0,
            post_process: None,
            effect_overlays: Vec::new(),
            named_effect_overlays: Vec::new(),
            effect_scene_texture: None,
            effect_scene_view: None,
            effect_scene_size: (0, 0),
            post_process_pipeline,
            post_process_layout,
            post_process_sampler,
            post_process_params,
            tonemap_layout,
            post_process_texture: None,
            post_process_scene_view: None,
            post_process_size: (0, 0),
            render_settings: RenderSettings::default(),
            hdr_warning_logged: false,
            hdr_scene_texture: None,
            hdr_scene_view: None,
            hdr_scene_size: (0, 0),
            hdr_world_sprite_pipeline,
            hdr_world_sprite_opaque_pipeline,
            hdr_world_sprite_additive_pipeline,
            tonemap_pipeline,
            tonemap_params,
            section_height_white,
            section_fields: std::collections::HashMap::new(),
            ssao_pipeline,
            ssao_bind_group_layout,
            ssao_params,
            ssao_blur_pipeline,
            ssao_blur_bind_group_layout,
            ssao_blur_params,
            ssao_raw_view,
            ssao_view,
            ssao_size: (1, 1),
            ssr_pipeline,
            ssr_bind_group_layout,
            ssr_params,
            ssr_blur_pipeline,
            ssr_blur_bind_group_layout,
            ssr_blur_params,
            ssr_raw_view,
            ssr_view,
            ssr_history_view,
            ssr_texture: ssr_blur_white,
            ssr_history_texture: ssr_history_white,
            ssr_prev_view_projection: None,
            tonemap_prev_view_projection: None,
            ssr_size: (1, 1),
            planar_reflection_texture,
            planar_reflection_view,
            planar_reflection_depth,
            planar_reflection_size: (1, 1),
            planar_camera_buffer,
            planar_camera_bind_group,
        })
    }
}

impl Renderer {
    /// P3-R02：在宿主首次打开 `RenderSettings.pbr` 时编译 PBR 网格变体（静态/蒙皮 × LDR/HDR）。
    ///
    /// 为什么不放进启动期无条件预编译：旧 3D 场景默认 `pbr=off`，它们不该为一份永远不用的
    /// 管线付启动时间。这里仍然不是"每帧补编译"——`pbr_pipelines_ready` 一旦置位，后续
    /// 帧只做缓存查找；重复 `set_render_settings` 也不会重新编译。
    ///
    /// 阴影变体与着色模型无关，不在这里重复创建；不支持存储缓冲的设备（WebGL2）改走
    /// `InstanceMode::Uniform` 的着色器变体，PBR 同样可用（材质布局的 binding 2 是 uniform）。
    pub(super) fn ensure_pbr_pipelines(&mut self) {
        if self.pbr_pipelines_ready {
            return;
        }
        self.pbr_pipelines_ready = true;
        let started = web_time::Instant::now();
        let hdr_supported = self.capabilities.hdr_usable();
        let ldr_only = [ColorTarget::Ldr];
        let both = [ColorTarget::Ldr, ColorTarget::Hdr];
        let targets: &[ColorTarget] = if hdr_supported { &both } else { &ldr_only };
        // 源文本在这里过 `variant_module`（覆盖目录优先），所以循环不能提前借用
        // `self.pipelines`：管线先在局部建好，再交给缓存。
        for skin in [SkinMode::Off, SkinMode::On] {
            for &target in targets {
                let key = ShaderVariantKey::mesh(Shading::Pbr, skin, target);
                let shader = self.variant_module(&key);
                let target_format = match target {
                    ColorTarget::Ldr => self.config.format,
                    ColorTarget::Hdr => wgpu::TextureFormat::Rgba16Float,
                };
                let label = format!("rustlua-{}-pipeline", key.label());
                let pipeline = Arc::new(create_mesh_pipeline(
                    &self.device,
                    &self.pbr_pipeline_layout,
                    &shader,
                    target_format,
                    &label,
                    true,
                    rustlua_render::mesh3d::MaterialAlphaMode::Opaque,
                    false,
                ));
                self.pipelines
                    .get_or_create(key, || Ok::<_, std::convert::Infallible>(pipeline))
                    .expect("PBR 变体数远低于 family/全局上限");
            }
        }
        let compile_ms = started.elapsed().as_secs_f64() * 1000.0;
        if compile_ms <= VARIANT_COMPILE_BUDGET_MS {
            tracing::info!(
                target: "game",
                "[render] PBR shader variants compiled on enable: compile_ms={compile_ms:.2} budget_ms={VARIANT_COMPILE_BUDGET_MS:.0}"
            );
        } else {
            tracing::warn!(
                target: "render",
                "[runtime] PBR shader 变体编译耗时 {compile_ms:.2}ms 超出预算 {VARIANT_COMPILE_BUDGET_MS:.0}ms；仅发生一次，请检查着色器复杂度/设备"
            );
        }
    }

    /// P3-R02-4 / P3-R03-2：按本帧实际出现的 alpha/双面/IBL 组合补编译管线。
    ///
    /// 启动期只预热默认的 opaque/单面/IBL-Off 组合；mask/blend/双面与 IBL 档位是场景内容
    /// 决定的，提前全编译（PBR 组合数会乘法增长）会越过 250ms 预算。这里每个组合只建一次，
    /// 之后和预热变体一样命中缓存。Legacy 着色忽略 alpha/双面/IBL（回滚档逐字节不变），
    /// 因此不补编译。
    ///
    /// `cutaway_both_sides`（P3-R211）：本帧真有会话裁剪平面、且封口档开了「吃到单面网格」
    /// 时置真，于是主通道的选键把双面强制为真。必须和 `passes.rs` 里主网格通道的选键用
    /// 同一个表达式，否则运行期会撞上「变体未在构造期预编译」的 panic。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ensure_frame_pipelines(
        &mut self,
        draws: &[FrameDraw],
        shading: Shading,
        target: ColorTarget,
        ibl: IblMode,
        cutaway_both_sides: bool,
    ) {
        if shading != Shading::Pbr {
            return;
        }
        use rustlua_render::mesh3d::MaterialAlphaMode;
        let target_format = match target {
            ColorTarget::Ldr => self.config.format,
            ColorTarget::Hdr => wgpu::TextureFormat::Rgba16Float,
        };
        let mut material_keys = Vec::new();
        let mut shadow_keys = Vec::new();
        for draw in draws {
            let skin = SkinMode::from(draw.material.skinned);
            // P3-R211：双面强制只影响主网格通道的选键；阴影通道仍按材质自身的双面档，
            // 否则剖面放回来的背面会进阴影图，把 P3-R54 起的阴影几何改掉。
            let double_sided = draw.material.double_sided || cutaway_both_sides;
            let mesh_key = ShaderVariantKey::mesh_with_alpha(
                shading,
                skin,
                target,
                draw.material.alpha_mode,
                double_sided,
            )
            .with_ibl(ibl);
            if !material_keys.contains(&mesh_key) {
                material_keys.push(mesh_key);
            }
            // P3-R170：平面反射 pass 用**双面**变体。镜像是保长反射（det = -1），
            // 同一个三角形在镜像相机下的屏幕绕序与主通道相反，用单面管线会被整片剔除
            // （像素门实测：单面 0 像素覆盖、双面 244/310）。水面上方的反射内容正反两面
            // 都要进图，因此按同样的 alpha/skin/IBL 档额外预热一份双面变体。
            if draw.material.alpha_mode != MaterialAlphaMode::Blend {
                let reflector_key = ShaderVariantKey::mesh_with_alpha(
                    shading,
                    skin,
                    target,
                    draw.material.alpha_mode,
                    true,
                )
                .with_ibl(ibl);
                if !material_keys.contains(&reflector_key) {
                    material_keys.push(reflector_key);
                }
            }
            if draw.material.alpha_mode != MaterialAlphaMode::Blend {
                let shadow_key = ShaderVariantKey::shadow_with_alpha(
                    skin,
                    draw.material.alpha_mode,
                    draw.material.double_sided,
                );
                if !shadow_keys.contains(&shadow_key) {
                    shadow_keys.push(shadow_key);
                }
            }
        }
        for key in material_keys {
            if self.pipelines.get(&key).is_some() {
                continue;
            }
            let shader = self.variant_module(&key);
            let label = format!("rustlua-{}-pipeline", key.label());
            self.pipelines
                .get_or_create(key, || {
                    Ok::<_, std::convert::Infallible>(Arc::new(create_mesh_pipeline(
                        &self.device,
                        &self.pbr_pipeline_layout,
                        &shader,
                        target_format,
                        &label,
                        true,
                        key.alpha,
                        key.double_sided,
                    )))
                })
                .expect("alpha/双面/IBL 网格变体数低于 family/全局上限");
            tracing::info!(
                target: "render",
                variant = %key.label(),
                "[render] mesh variant compiled on demand"
            );
        }
        for key in shadow_keys {
            if self.pipelines.get(&key).is_some() {
                continue;
            }
            let shader = self.variant_module(&key);
            let label = format!("rustlua-{}-pipeline", key.label());
            self.pipelines
                .get_or_create(key, || {
                    Ok::<_, std::convert::Infallible>(Arc::new(create_shadow_pipeline(
                        &self.device,
                        &self.shadow_pipeline_layout,
                        &shader,
                        &label,
                        key.double_sided,
                    )))
                })
                .expect("alpha/双面阴影变体数低于 family/全局上限");
            tracing::info!(
                target: "render",
                variant = %key.label(),
                "[render] shadow alpha variant compiled on demand"
            );
        }
    }

    /// P3-R05：变体用的源文本——开发期覆盖目录优先，否则内置源。
    ///
    /// 覆盖文件**读不出来**时这里记一条 warning 并回退内置源：构造期还没有"旧管线"可保留，
    /// 而运行期的重载走 [`Renderer::set_shader_override_dir`]，那里把错误原样报给作者。
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn variant_source(
        &self,
        root: &'static ShaderSource,
    ) -> (&'static str, Cow<'static, str>) {
        match self.shader_library.resolve(root.name) {
            Ok(Some(resolved)) => (resolved.repo_path, resolved.source),
            Ok(None) => (root.path, Cow::Borrowed(root.source)),
            Err(error) => {
                tracing::warn!(target: "render", "[render] {error}；本次沿用内置源");
                (root.path, Cow::Borrowed(root.source))
            }
        }
    }

    /// wasm/release：恒为内置源。覆盖目录在这两个平台上没有入口（计划 §6.5 的发布门），
    /// 因此也不该把文件 IO 分支编进浏览器产物。
    #[cfg(target_arch = "wasm32")]
    pub(super) fn variant_source(
        &self,
        root: &'static ShaderSource,
    ) -> (&'static str, Cow<'static, str>) {
        let _ = &self;
        (root.path, Cow::Borrowed(root.source))
    }

    /// 延迟补编译/重载路径的着色器模块：与启动期共用 [`variant_shader_module`] 的预处理口径，
    /// 只是源文本先过 [`Renderer::variant_source`]。
    ///
    /// 标签按**变体键**命名（`rustlua-<变体>-shader`）：重载后要能在诊断里认出是哪个变体用了
    /// 哪份源，共用一个标签就查不出来了。
    pub(super) fn variant_module(&self, key: &ShaderVariantKey) -> wgpu::ShaderModule {
        let root = variant_root(key);
        let label = format!("rustlua-{}-shader", key.label());
        let (path, source) = self.variant_source(root);
        let instance_mode = self.instance_mode();
        let preprocessed =
            match preprocessed_variant(path, &source, key.skin, key.ibl, instance_mode) {
                Ok(preprocessed) => preprocessed,
                Err(error) => {
                    // 覆盖文件里 `//#if` 拼错时，延迟补编译（首次打开 PBR 那一帧）正好撞上它。
                    // panic 会把整个进程带走；这里沿用内置源并报 warning，逐变体的错误文本由
                    // `set_shader_override_dir` 那条路负责返回。
                    tracing::warn!(
                        target: "render",
                        "[render] {path} 预处理失败：{error}；本次沿用内置源"
                    );
                    preprocessed_variant(root.path, root.source, key.skin, key.ibl, instance_mode)
                        .unwrap_or_else(|error| panic!("内置源 {} 预处理失败：{error}", root.path))
                }
            };
        module_from_preprocessed(&self.device, &preprocessed, &label)
    }

    /// 本设备逐实例模型矩阵的取法（能力在构造期采一次，之后恒定）。
    pub(super) const fn instance_mode(&self) -> InstanceMode {
        InstanceMode::from_storage_capable(self.storage_capable)
    }
}

/// 变体键 → 着色器模块：先过预处理器，只保留该变体选中的分支。
///
/// 预处理失败属于构建期不变量（feature 拼写/指令配对由单测钉住），所以这里直接 panic，
/// 而不是把错误拖到第一次绘制。
pub(super) fn variant_shader_module(
    device: &wgpu::Device,
    shader: &ShaderSource,
    skin: SkinMode,
    ibl: IblMode,
    instance_mode: InstanceMode,
    label: &str,
) -> wgpu::ShaderModule {
    variant_shader_module_from(
        device,
        shader.path,
        shader.source,
        skin,
        ibl,
        instance_mode,
        label,
    )
}

/// 上一函数的另一半：根 shader 的**源文本由调用方给**（P3-R05 的覆盖目录走这条路）。
///
/// 拆成两半的理由：内置源在编译期就固定（`include_str!`），覆盖源要到运行期才读得出来，
/// 但两条路必须共用**同一份预处理口径**（feature 表、known_features、行号映射），否则
/// "覆盖版变体"和"内置版变体"画的不是同一个程序。
pub(super) fn variant_shader_module_from(
    device: &wgpu::Device,
    path: &str,
    source: &str,
    skin: SkinMode,
    ibl: IblMode,
    instance_mode: InstanceMode,
    label: &str,
) -> wgpu::ShaderModule {
    let preprocessed = preprocessed_variant(path, source, skin, ibl, instance_mode)
        .unwrap_or_else(|error| panic!("{path} 预处理失败：{error}"));
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(preprocessed.source().to_owned().into()),
    })
}

/// 预处理结果 → 模块（热重载的提交路径）。
///
/// 与 [`variant_shader_module_from`] 的区别只在于**谁负责处理预处理失败**：构造期的内置源
/// 出错是仓库 bug，panic 是对的；热重载的出错要在 GPU 之外变成报告并保留旧管线，所以那条路
/// 先拿到已校验的 [`PreprocessedShader`]，这里不再重复预处理（重复一次就多一次 panic 分支）。
pub(super) fn module_from_preprocessed(
    device: &wgpu::Device,
    preprocessed: &PreprocessedShader,
    label: &str,
) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(preprocessed.source().to_owned().into()),
    })
}

/// 2D 叠层顶点布局（`OverlayVertex`：位置 2 + UV 2 + 颜色 4 + 纹理槽 u32）。
fn overlay_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
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
    }
}

/// 矢量文字顶点布局（位置 2 + UV 2 + 字形索引 u32 + 颜色 4）。
fn vector_text_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
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
    }
}

/// 世界 Sprite 顶点布局（位置 3 + UV 2 + 颜色 4）。
fn world_sprite_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
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
    }
}

/// 把三个 2D 批次的公共骨架收成一个函数：它们只差顶点布局、深度策略与标签。
///
/// 构造期与热重载期共用它，"重载后的 2D 管线"和"首帧的 2D 管线"才是同一份描述符；否则
/// 覆盖文件能过画面、重启后行为却变了，是最难查的一种漂移。
#[allow(clippy::too_many_arguments)] // 描述符字段就是这么多，拆结构体只会多一层搬运
fn two_d_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    vertex_buffers: &[wgpu::VertexBufferLayout<'static>],
    topology: wgpu::PrimitiveTopology,
    format: wgpu::TextureFormat,
    depth: Option<wgpu::DepthStencilState>,
    label: &str,
) -> wgpu::RenderPipeline {
    two_d_pipeline_with_blend(
        device,
        layout,
        module,
        vertex_buffers,
        topology,
        format,
        depth,
        wgpu::BlendState::ALPHA_BLENDING,
        label,
    )
}

/// 与 [`two_d_pipeline`] 相同，只把混合状态参数化（M84 P2-③）。
///
/// 为什么要单独一支而不是给原函数加参数：原函数的 6 个调用方全是"标准 alpha 混合"，
/// 给它们各加一个 `BlendState::ALPHA_BLENDING` 只是噪声。Spine 的 additive/multiply 是
/// 少数派，单独走这条路径，语义差别因此只出现在调用点。
#[allow(clippy::too_many_arguments)] // 同 `two_d_pipeline`
fn two_d_pipeline_with_blend(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    vertex_buffers: &[wgpu::VertexBufferLayout<'static>],
    topology: wgpu::PrimitiveTopology,
    format: wgpu::TextureFormat,
    depth: Option<wgpu::DepthStencilState>,
    blend: wgpu::BlendState,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: vertex_buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: depth,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// `textured.wgsl` 的叠层管线。`pipeline` 与 `pipeline_sdf` 只差标签：SDF 与非 SDF 用的
/// 采样器在绘制时绑定，管线本身逐字段相同。
pub(super) fn build_textured_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    two_d_pipeline(
        device,
        layout,
        module,
        &[overlay_vertex_layout()],
        wgpu::PrimitiveTopology::TriangleList,
        format,
        None,
        label,
    )
}

/// `vector_text.wgsl` 的管线（存储缓冲缺失时调用方给占位管线，不走这里）。
pub(super) fn build_vector_text_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    two_d_pipeline(
        device,
        layout,
        module,
        &[vector_text_vertex_layout()],
        wgpu::PrimitiveTopology::TriangleList,
        format,
        None,
        label,
    )
}

/// `world_sprite.wgsl` 的管线；`pass` 决定深度策略，`format` 决定 LDR/HDR 变体。
pub(super) fn build_world_sprite_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    pass: DepthPass,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    two_d_pipeline(
        device,
        layout,
        module,
        &[world_sprite_vertex_layout()],
        wgpu::PrimitiveTopology::TriangleList,
        format,
        Some(depth_stencil_state(pass, wgpu::DepthBiasState::default())),
        label,
    )
}

/// 与 [`build_world_sprite_pipeline`] 相同，只把混合状态参数化（M84 P2-③ 的 additive 变体）。
pub(super) fn build_world_sprite_pipeline_with_blend(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    pass: DepthPass,
    format: wgpu::TextureFormat,
    blend: wgpu::BlendState,
    label: &str,
) -> wgpu::RenderPipeline {
    two_d_pipeline_with_blend(
        device,
        layout,
        module,
        &[world_sprite_vertex_layout()],
        wgpu::PrimitiveTopology::TriangleList,
        format,
        Some(depth_stencil_state(pass, wgpu::DepthBiasState::default())),
        blend,
        label,
    )
}

/// Spine additive 混合的 wgpu 方程。
///
/// 与官方 runtime 的 additive 语义对齐：`dst = src*src_alpha + dst`（颜色相加、不衰减
/// 目标），alpha 通道同样相加。这是 Spine 的 `additive` 在 GL 侧的标准写法
/// （`GL_SRC_ALPHA, GL_ONE`），与 `BlendState::PREMULTIPLIED_ALPHA_BLENDING` 不同——
/// 后者会乘进目标 alpha，画光效时会出现"越亮越透"的错误观感。
#[must_use]
pub(super) fn additive_blend_state() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

/// GPU 粒子（`gpu_particles.rs` 内联 draw 段）的 instanced 管线。
///
/// 与世界 Sprite 管线只差两处：**没有顶点缓冲**（几何由 `@builtin(vertex_index)` 的四角给出，
/// 逐实例数据走 group2 的只读存储缓冲）与 **triangle strip**（4 顶点/实例，旧实现是 6 顶点
/// triangle list）。深度策略、alpha 混合、片元输出全部一致——粒子本来就是半透明段的一种。
pub(super) fn build_gpu_particle_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    _pass: DepthPass,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    two_d_pipeline(
        device,
        layout,
        module,
        &[],
        wgpu::PrimitiveTopology::TriangleStrip,
        format,
        None,
        label,
    )
}

/// 无顶点缓冲的全屏三角管线（`post_process.wgsl` 与 `tonemap.wgsl` 共用：顶点由
/// `@builtin(vertex_index)` 生成，不开混合也不读深度）。
pub(super) fn build_fullscreen_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module,
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
    })
}

/// 变体轴 → 预处理后的源（带行号映射）。
///
/// 热重载先走这里而不是直接建模块：预处理失败要**报告并保留旧管线**，不能 panic。
/// 返回的 [`PreprocessedShader::line_map`] 还把出错行映射回原文件，编辑器 Console 因此能指到
/// 覆盖目录里那一行的真实行号。
pub(super) fn preprocessed_variant(
    path: &str,
    source: &str,
    skin: SkinMode,
    ibl: IblMode,
    instance_mode: InstanceMode,
) -> Result<PreprocessedShader, PreprocessError> {
    let mut features = Vec::with_capacity(
        skin.features().len() + ibl.features().len() + instance_mode.features().len(),
    );
    features.extend_from_slice(skin.features());
    features.extend_from_slice(ibl.features());
    features.extend_from_slice(instance_mode.features());
    preprocess(
        path,
        source,
        PreprocessOptions {
            features: &features,
            known_features: KNOWN_FEATURES,
            ..Default::default()
        },
    )
}

/// 预处理器认识的 feature 全集（与 `//#if` 指令同源；拼错的名字按未知 feature 报错）。
pub(super) const KNOWN_FEATURES: &[&str] =
    &["SKIN", "IBL_DIFFUSE", "IBL_SPECULAR", "STORAGE_INSTANCES"];

/// 变体键 → 根 shader（重建与接口比对都从这张表出发）。
///
/// 阴影与着色模型无关（[`ShaderVariantKey::shadow`] 恒 Legacy），PBR 是第二份片元着色，
/// 所以只有 `Mesh3d + Pbr` 落到 `mesh3d_pbr.wgsl`。
pub(super) fn variant_root(key: &ShaderVariantKey) -> &'static ShaderSource {
    match (key.family, key.shading) {
        (PipelineFamily::Shadow, _) => &SHADOW,
        (PipelineFamily::Mesh3d, Shading::Legacy) => &MESH3D,
        (PipelineFamily::Mesh3d, Shading::Pbr) => &MESH3D_PBR,
    }
}

const fn skin_tag(skin: SkinMode) -> &'static str {
    match skin {
        SkinMode::Off => "static",
        SkinMode::On => "skin",
    }
}

/// 与 `rustlua_render::mesh3d::MeshVertex` 的 `#[repr(C)]` 布局逐字段对应。
const MESH_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 6] = [
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
    // 切线与骨骼属性刻意保持"旧位置不动、新属性追加"：旧网格/阴影着色器只声明到
    // location 4，多出来的属性不会被消费，`pbr=off` 的像素因此不受影响。
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 32,
        shader_location: 5,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Uint32x4,
        offset: 48,
        shader_location: 3,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 64,
        shader_location: 4,
    },
];

fn mesh_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<rustlua_render::mesh3d::MeshVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &MESH_VERTEX_ATTRIBUTES,
    }
}

/// P3-R02：逐实例法线矩阵（`transpose(inverse(mat3(model)))`）的顶点缓冲槽 1。
/// 每实例 4 个 `vec4`，与着色器里的 `mat4x4<f32>` 列主序一一对应。
const PBR_NORMAL_ATTRIBUTES: [wgpu::VertexAttribute; 4] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 0,
        shader_location: 6,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 16,
        shader_location: 7,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 32,
        shader_location: 8,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 48,
        shader_location: 9,
    },
];

fn pbr_normal_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: 64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &PBR_NORMAL_ATTRIBUTES,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn create_mesh_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    label: &str,
    pbr_normals: bool,
    alpha: rustlua_render::mesh3d::MaterialAlphaMode,
    double_sided: bool,
) -> wgpu::RenderPipeline {
    use rustlua_render::mesh3d::MaterialAlphaMode;
    let buffers = if pbr_normals {
        vec![mesh_vertex_layout(), pbr_normal_layout()]
    } else {
        vec![mesh_vertex_layout()]
    };
    let mut depth = depth_stencil_state(DepthPass::Mesh3d, wgpu::DepthBiasState::default());
    // 半透明不写深度：否则后画的透明片会挡住更远的透明片；深度测试仍然保留。
    if alpha == MaterialAlphaMode::Blend {
        depth.depth_write_enabled = false;
    }
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(if alpha == MaterialAlphaMode::Blend {
                    wgpu::BlendState::ALPHA_BLENDING
                } else {
                    wgpu::BlendState::REPLACE
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: if double_sided {
                None
            } else {
                Some(wgpu::Face::Back)
            },
            ..Default::default()
        },
        depth_stencil: Some(depth),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
}

pub(super) fn create_shadow_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    double_sided: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[mesh_vertex_layout()],
        },
        // mask 材质需要在片元阶段按贴图 alpha 剔除；opaque 分支不采样，深度结果不变。
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: if double_sided {
                None
            } else {
                Some(wgpu::Face::Back)
            },
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
    })
}

/// 设备能力不足时给"吃存储缓冲"的管线槽位填一条**占位管线**（目前只剩矢量文字）。
///
/// 为什么要占位而不是把字段改成 `Option`：那个字段在 `passes.rs`、`gpu_texture.rs`、
/// `arena.rs`、`world_sprite.rs` 和全部原生测试里被引用一百多次，`Option` 化会让整个渲染器
/// 的签名扩散开，而收益只是"少画一条本来就不该画的路径"。占位管线是合法对象、几乎零成本，
/// 而且**不可能**画出看起来对的东西：顶点着色器只吐一个退化点，片元写全 0（含 alpha），
/// 即使被误绑也只是什么都没画。
///
/// 它永远不该被绑定——由 `passes.rs` 的 `storage_capable` 守卫保证（能力不足时计划里就没有
/// 矢量文字通道，同时把丢弃的 draw 记进 `dropped_draws`）。3D 网格/阴影不再需要占位：
/// 它们在没有存储缓冲时走 `@binding(2)` 的 uniform 模型矩阵（见 `InstanceMode`）。
fn placeholder_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    const PLACEHOLDER_SHADER: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}
"#;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(PLACEHOLDER_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
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
    })
}

#[cfg(test)]
mod additive_blend_tests {
    //! M84 P2-③：Spine additive 混合方程的契约。
    //!
    //! 钉的是**混合方程本身**——它是画面观感的唯一来源，写错了表现为"光效越亮越透"或
    //! "光效被背景吃掉"，两者单看代码都像对的。与官方 runtime 对应：
    //! Spine 的 `additive` 在 GL 侧是 `glBlendFunc(GL_SRC_ALPHA, GL_ONE)`。

    use super::additive_blend_state;

    #[test]
    fn additive_adds_to_a_non_fading_destination() {
        let blend = additive_blend_state();
        assert_eq!(blend.color.src_factor, wgpu::BlendFactor::SrcAlpha);
        assert_eq!(blend.color.dst_factor, wgpu::BlendFactor::One);
        assert_eq!(blend.color.operation, wgpu::BlendOperation::Add);
        // alpha 通道同样相加：Spine 的光效合成不依赖目标 alpha，
        // 乘进目标会变成"越亮越透"。
        assert_eq!(blend.alpha.src_factor, wgpu::BlendFactor::SrcAlpha);
        assert_eq!(blend.alpha.dst_factor, wgpu::BlendFactor::One);
        assert_eq!(blend.alpha.operation, wgpu::BlendOperation::Add);
    }

    /// 反面对照：additive 必须与标准 alpha 混合可区分。
    ///
    /// 若哪天有人把它写成 `ALPHA_BLENDING` 的别名，这条会先红——光效观感的问题
    /// 不该靠肉眼看截图才发现。
    #[test]
    fn additive_is_distinguishable_from_standard_alpha_blending() {
        let additive = additive_blend_state();
        let alpha = wgpu::BlendState::ALPHA_BLENDING;
        assert_ne!(
            additive.color.dst_factor, alpha.color.dst_factor,
            "additive 的目标因子是 One（不衰减目标），标准 alpha 是 OneMinusSrcAlpha"
        );
    }
}
