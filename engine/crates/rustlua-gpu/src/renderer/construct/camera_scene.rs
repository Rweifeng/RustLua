//! 相机、光照与初始 uniform 缓冲（拆分批 D / 基础管线）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里从 `initial_viewport` 到 `identity_palette` 结束
//! （**114 行、10 处资源创建**，14 个产出）。
//!
//! ## 依赖面（实测，很干净）
//!
//! **3 个输入**：`device` / `config`（取窗口尺寸算视口）/ `shadow_resolution`（阴影级联分辨率）。
//!
//! 探查时以为它还要 B 的 `sampler`（因为 `shadow_sampler` / `point_shadow_plain_sampler`
//! 长得像派生品），实际两个都是**用自己的参数现建**的，不借 `sampler`。
//! 也不依赖 A 的 `camera_bind_group_layout`——它只造 layout 的**消费者**（bind group 是 H）。
//!
//! ## 语义零变化的等价点
//!
//! 1. **初始相机是硬编码的固定值**（位置 `(0,0,5)`、`FRAC_PI_2` 视角、近 0.1 远 100）
//!    ——这是"渲染器刚构造出来时"的那一帧，不是任何场景的相机。不要"顺手"参数化。
//! 2. **光照方向约定**：`Light::default()` 的 `direction` 是**光传播方向**
//!    （所以太阳的 y 分量为负）。写反了会得到"从地下打上来的太阳"。
//! 3. **阴影相机矩阵复制 4 份**（`shadow_camera_buffer` 与四个
//!    `shadow_pass_camera_buffers`）——远/中/近/超四档级联初始共用同一个矩阵，
//!    由 `write_light_uniforms` 每帧重写。这里给的是"还没分配级联时"的初值。
//! 4. `shadow_views` 是 `create_shadow_view` 的第二个返回值，跟着一起交出去。

use wgpu::util::DeviceExt as _;
use wgpu::{Buffer, Device, Sampler, SurfaceConfiguration};

use rustlua_camera::camera::{Camera3D, Viewport};
use rustlua_math::math3d::{Mat4, Vec3};

use crate::renderer::{Renderer, camera_uniform, identity_palette_data};

/// 相机、光照与初始 uniform 缓冲（14 个）。
pub(crate) struct CameraScene {
    pub initial_viewport: Viewport,
    pub camera: Camera3D,
    pub camera_matrix: Mat4,
    pub light_camera: Camera3D,
    pub light_view: Mat4,
    pub light_projection: Mat4,
    pub light_matrix: Mat4,
    pub shadow_sampler: Sampler,
    pub point_shadow_plain_sampler: Sampler,
    pub shadow_sample_view: wgpu::TextureView,
    pub shadow_views: [wgpu::TextureView; 4],
    pub camera_buffer: Buffer,
    pub light_buffer: Buffer,
    pub shadow_camera_buffer: Buffer,
    pub shadow_pass_camera_buffers: [Buffer; 4],
    pub identity_palette: Buffer,
}

/// 建初始相机、光照与对应的 uniform 缓冲。
pub(crate) fn camera_scene(
    device: &Device,
    config: &SurfaceConfiguration,
    shadow_resolution: u32,
) -> anyhow::Result<CameraScene> {
    let initial_viewport = Viewport::new(config.width as f32, config.height as f32)?;
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 0.0, 5.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )?;
    let camera_matrix = camera.view_projection_matrix(initial_viewport)?;
    let light_camera = Camera3D::perspective(Vec3::new(4.0, 6.0, 4.0), Vec3::ZERO, 1.3, 0.1, 30.0)?;
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
    let (shadow_sample_view, shadow_views) =
        Renderer::create_shadow_view(device, shadow_resolution);
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
    let point_shadow_plain_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("rustlua-point-shadow-plain-sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("rustlua-camera3d-buffer"),
        contents: bytemuck::cast_slice(&camera_uniform(
            &camera_matrix,
            camera.position,
            0.0,
            camera.clip_plane,
            0.5,
            1.0,
            100.0,
            0.26,
            [1.0, 1.0, 1.0, 0.0],
            [0.0, 0.0],
            [0.0, 1.5],
            [0.0, 1.5, 0.0, 1.5],
            0.0,
        )),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let light_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("rustlua-light-buffer"),
        // 与 `Light::default()` 一致：`direction` 是**光传播方向**，所以天空里的太阳 y 分量为负；
        // 点光数组先全部补零，由 `write_light_uniforms` 每帧重写。
        contents: &crate::renderer::light_uniform_bytes(
            rustlua_render::light::Light::default(),
            &[],
            false,
            false,
            None,
        ),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let shadow_camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("rustlua-shadow-camera-buffer"),
        contents: bytemuck::cast_slice(
            &[
                light_matrix.columns,
                light_matrix.columns,
                light_matrix.columns,
                light_matrix.columns,
            ]
            .concat(),
        ),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let shadow_pass_camera_buffers = [
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-shadow-pass-camera-near"),
            contents: bytemuck::cast_slice(&light_matrix.columns),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-shadow-pass-camera-mid"),
            contents: bytemuck::cast_slice(&light_matrix.columns),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-shadow-pass-camera-far"),
            contents: bytemuck::cast_slice(&light_matrix.columns),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-shadow-pass-camera-ultra"),
            contents: bytemuck::cast_slice(&light_matrix.columns),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        }),
    ];
    let identity_palette = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("rustlua-identity-joint-palette"),
        contents: bytemuck::cast_slice(&identity_palette_data()),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    Ok(CameraScene {
        initial_viewport,
        camera,
        camera_matrix,
        light_camera,
        light_view,
        light_projection,
        light_matrix,
        shadow_sampler,
        point_shadow_plain_sampler,
        shadow_sample_view,
        shadow_views,
        camera_buffer,
        light_buffer,
        shadow_camera_buffer,
        shadow_pass_camera_buffers,
        identity_palette,
    })
}
