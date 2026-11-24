//! 环境光照 group（拆分批 J / 基础管线）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里 `P3-R03` 那段（21 行）。它绝大部分工作已由 `renderer/environment.rs`
//! 承担，这里只是把这些辅助函数串起来 —— 所以它一直是"最该搬但收益最小"的一段。
//!
//! ## 依赖面
//!
//! 3 个输入：`device` / `queue` / `capabilities.float_linear_filter`。
//!
//! ## 为什么绑"全零 SH + 1×1 黑 cube"而不是空
//!
//! 即使 IBL 关闭也绑一份占位：**PBR 管线布局只有一种**，不需要为 Off/On 切换布局。
//! 绑定组形状恒定，是这个渲染器反复用的手法（SSAO/SSR 的占位图同理）。
//!
//! ## Tier B 的处理
//!
//! 浮点线性过滤不可用时按 nearest / 不可过滤声明，避免绑定校验失败
//! ——这个判定透传给 layout 与 sampler 两处，必须是同一个值。

use wgpu::util::DeviceExt as _;
use wgpu::{BindGroup, BindGroupLayout, Buffer, Device, Queue, Sampler, TextureView};

use rustlua_render::render_settings::{EnvironmentLighting, IblMode};

use crate::renderer::RendererCapabilities;

/// 环境光照 group 的全部资源。
pub(crate) struct EnvironmentGroup {
    pub bind_group_layout: BindGroupLayout,
    pub buffer: Buffer,
    pub fallback_view: TextureView,
    pub sampler: Sampler,
    pub view: TextureView,
    pub bind_group: BindGroup,
}

/// 建环境光照 group（初始状态是"全零 SH + 1×1 黑 cube"）。
pub(crate) fn environment_group(
    device: &Device,
    queue: &Queue,
    capabilities: &RendererCapabilities,
) -> EnvironmentGroup {
    // P3-R03：环境 group（uniform + 预过滤 cubemap + 采样器）。即使 IBL 关闭也绑定一份
    // 全零 SH 与 1×1 黑 cube，这样 PBR 管线布局只有一种，不需要为 Off/On 切换布局。
    // 浮点线性过滤不可用时按 nearest/不可过滤声明（Tier B），避免绑定校验失败。
    let environment_bind_group_layout =
        crate::renderer::environment::bind_group_layout(device, capabilities.float_linear_filter);
    let environment_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("rustlua-environment-buffer"),
        contents: bytemuck::cast_slice(&EnvironmentLighting::default().uniform(IblMode::Off)),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let environment_fallback_view = crate::renderer::environment::fallback_view(device, queue);
    let environment_sampler =
        crate::renderer::environment::sampler(device, capabilities.float_linear_filter);
    let environment_view = environment_fallback_view.clone();
    let environment_bind_group = crate::renderer::environment::bind_group(
        device,
        &environment_bind_group_layout,
        &environment_buffer,
        &environment_view,
        &environment_sampler,
    );
    EnvironmentGroup {
        bind_group_layout: environment_bind_group_layout,
        buffer: environment_buffer,
        fallback_view: environment_fallback_view,
        sampler: environment_sampler,
        view: environment_view,
        bind_group: environment_bind_group,
    }
}
