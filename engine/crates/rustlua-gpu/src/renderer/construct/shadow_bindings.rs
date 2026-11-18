//! 阴影绑定组（拆分批 I / 基础管线）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里的 `shadow_bind_group_layout` + `shadow_pass_bind_groups` +
//! `point_shadow_bind_groups`（29 行、3 处创建）。
//!
//! ## 依赖面
//!
//! `shadow_pass_camera_buffers`（拆分批 D 的产物）与 `point_shadow_camera_buffers`
//! （拆分批 F 的产物）——两者都已就位，所以这一批现在才能做。
//!
//! ## 为什么一个 layout 服务两种绑定组
//!
//! 方向光级联（4 个）与点光立方体面（8 层）用的是**同一个 layout**：两者都只要
//! 一个 64 字节的 uniform（`binding 0`，`min_binding_size: 64` = 一个完整的 mat4）。
//! 只是每个绑定组指向不同的 buffer —— 所以复用一个 layout，而不是建两个。
//!
//! 这个 layout 之后还会被 `shadow_pipeline_layout`（拆分批 K）用，所以必须交出去。

use wgpu::{BindGroup, BindGroupLayout, Device};

use crate::renderer::POINT_SHADOW_LAYERS;

/// 阴影的 layout 与两组绑定。
pub(crate) struct ShadowBindings {
    pub layout: BindGroupLayout,
    /// 四个方向光级联各一个。
    pub cascade_bind_groups: [BindGroup; 4],
    /// 每个点光一层（每层是立方的 6 个面）。
    pub point_bind_groups: [BindGroup; POINT_SHADOW_LAYERS],
}

/// 建阴影布局与两类绑定组。
pub(crate) fn shadow_bindings(
    device: &Device,
    shadow_pass_camera_buffers: &[wgpu::Buffer; 4],
    point_shadow_camera_buffers: &[wgpu::Buffer; POINT_SHADOW_LAYERS],
) -> ShadowBindings {
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
    let shadow_pass_bind_groups = [0, 1, 2, 3].map(|cascade| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-shadow-pass-bind-group"),
            layout: &shadow_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shadow_pass_camera_buffers[cascade].as_entire_binding(),
            }],
        })
    });
    let point_shadow_bind_groups: [wgpu::BindGroup; POINT_SHADOW_LAYERS] =
        std::array::from_fn(|layer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rustlua-point-shadow-pass-bind-group"),
                layout: &shadow_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: point_shadow_camera_buffers[layer].as_entire_binding(),
                }],
            })
        });
    ShadowBindings {
        layout: shadow_bind_group_layout,
        cascade_bind_groups: shadow_pass_bind_groups,
        point_bind_groups: point_shadow_bind_groups,
    }
}
