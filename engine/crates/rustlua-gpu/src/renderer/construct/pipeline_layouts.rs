//! 3D 通道的三条管线布局（拆分批 K / 基础布局）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里 `mesh_pipeline_layout` / `pbr_pipeline_layout` /
//! `shadow_pipeline_layout`（22 行、3 处创建）。
//!
//! ## 依赖面：四个已拆好的布局
//!
//! 它们全部来自之前的批次，所以这一批可以"只做拼装"：
//!
//! | 依赖 | 批次 |
//! |---|---|
//! | `camera_bind_group_layout` | A |
//! | `material_bind_group_layout` | C |
//! | `environment_bind_group_layout` | J |
//! | `shadow_bind_group_layout` | I |
//!
//! ## 三条布局的差别就是"绑哪几组"
//!
//! | 布局 | group 组合 |
//! |---|---|
//! | `mesh`（Legacy） | camera + material |
//! | `pbr` | camera + material + environment |
//! | `shadow` | shadow + material |
//!
//! ## 语义零变化的等价点
//!
//! 1. **Legacy 保持两组**：多一组会让旧着色器绑定不上，也会让 `pbr=off`
//!    的像素门失去"逐字节不变"的前提。这是个回滚档，不是"还没升级"。
//! 2. **pbr 把 environment 放第三组**：组序（0/1/2）是着色器的 `@group` 契约，
//!    重排会让所有 PBR 着色器绑定错位。
//! 3. **shadow 用 shadow 自己的 layout 当 group0**，不用 camera —— 阴影 pass 只需要
//!    光照矩阵，不需要相机。
//!
//! ## 下游
//!
//! 这三条是变体编译（拆分批 L）的输入：`create_mesh_pipeline` /
//! `create_shadow_pipeline` 都要拿对应的 layout。

use wgpu::{BindGroupLayout, Device, PipelineLayout};

/// 三条管线布局。
pub(crate) struct PipelineLayouts {
    pub mesh: PipelineLayout,
    pub pbr: PipelineLayout,
    pub shadow: PipelineLayout,
}

/// 建 3D 通道的三条管线布局。
pub(crate) fn pipeline_layouts(
    device: &Device,
    camera_bind_group_layout: &BindGroupLayout,
    material_bind_group_layout: &BindGroupLayout,
    environment_bind_group_layout: &BindGroupLayout,
    shadow_bind_group_layout: &BindGroupLayout,
) -> PipelineLayouts {
    let mesh_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rustlua-mesh3d-pipeline-layout"),
        bind_group_layouts: &[camera_bind_group_layout, material_bind_group_layout],
        push_constant_ranges: &[],
    });
    // 回滚档（Legacy）保持两组布局：多一组会让旧着色器绑定不上，也会让 `pbr=off`
    // 的像素门失去"逐字节不变"的前提。
    let pbr_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rustlua-mesh3d-pbr-pipeline-layout"),
        bind_group_layouts: &[
            camera_bind_group_layout,
            material_bind_group_layout,
            environment_bind_group_layout,
        ],
        push_constant_ranges: &[],
    });
    let shadow_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rustlua-shadow-pipeline-layout"),
        bind_group_layouts: &[shadow_bind_group_layout, material_bind_group_layout],
        push_constant_ranges: &[],
    });
    PipelineLayouts {
        mesh: mesh_pipeline_layout,
        pbr: pbr_pipeline_layout,
        shadow: shadow_pipeline_layout,
    }
}
