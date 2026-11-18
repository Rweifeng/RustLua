//! 3D 变体的启动期预编译（拆分批 L / 最后一块）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里从 `InstanceMode::from_storage_capable` 到超预算告警结束
//! （**83 行**）。它没有直接的 `device.create_*`——所有管线都在
//! `PipelineCache::get_or_create` 的闭包里建。
//!
//! ## 依赖面
//!
//! `device` / `format` / `capabilities` / `storage_capable` /
//! `mesh_pipeline_layout` 与 `shadow_pipeline_layout`（批次 K 的产物）/ `pipeline`（批次 B）。
//!
//! ## 语义零变化的等价点
//!
//! 1. **HDR 变体只 `capabilities.hdr_usable()` 时存在**：`hdr_format` 是 `Option`，
//!    只加到 `targets` 列表里；能力不足时列表只有 LDR 一项。
//! 2. **两条实现路径都编译真管线**：实例矩阵取法由 `instance_mode` 决定
//!    （storage 数组 / 64 字节 uniform），WebGL2 没有存储缓冲但画的是同一批网格。
//! 3. **超预算只告警一次，不取消**：此刻管线已经建好，取消只会把同样的卡顿挪到首帧。
//!    这条是刻意行为——不要在"优化"时把它改成 cancel。
//! 4. `variant_stats` 之后随资源快照暴露给 Profiler，是启动性能的可观测口。

use std::sync::Arc;

use wgpu::{Device, RenderPipeline, TextureFormat};

use crate::renderer::RendererCapabilities;
use crate::renderer::pipeline_cache::{
    ColorTarget, InstanceMode, PipelineCache, PipelineCacheStats, ShaderVariantKey, Shading,
    SkinMode, VARIANT_COMPILE_BUDGET_MS,
};
use crate::shader::{MESH3D, SHADOW};
use rustlua_render::render_settings::IblMode;

use super::{create_mesh_pipeline, create_shadow_pipeline, skin_tag, variant_shader_module};

/// 预编译的变体缓存与统计。
pub(crate) struct Variants {
    pub instance_mode: InstanceMode,
    pub pipelines: PipelineCache<Arc<RenderPipeline>>,
    pub hdr_format: Option<TextureFormat>,
    pub stats: PipelineCacheStats,
}

/// 预编译 3D 网格与阴影的全部变体（LDR/HDR × 蒙皮开/关）。
pub(crate) fn compile_variants(
    device: &Device,
    format: TextureFormat,
    capabilities: &RendererCapabilities,
    storage_capable: bool,
    mesh_pipeline_layout: &wgpu::PipelineLayout,
    shadow_pipeline_layout: &wgpu::PipelineLayout,
) -> Variants {
    let instance_mode = InstanceMode::from_storage_capable(storage_capable);
    let mut pipelines: PipelineCache<Arc<wgpu::RenderPipeline>> = PipelineCache::new();
    let hdr_format = capabilities
        .hdr_usable()
        .then_some(wgpu::TextureFormat::Rgba16Float);
    for skin in [SkinMode::Off, SkinMode::On] {
        let shader = variant_shader_module(
            &device,
            &MESH3D,
            skin,
            IblMode::Off,
            instance_mode,
            &format!("rustlua-mesh3d-{}-shader", skin_tag(skin)),
        );
        let targets = if let Some(hdr) = hdr_format {
            vec![(ColorTarget::Ldr, format), (ColorTarget::Hdr, hdr)]
        } else {
            vec![(ColorTarget::Ldr, format)]
        };
        for (target, target_format) in targets {
            let key = ShaderVariantKey::mesh(Shading::Legacy, skin, target);
            let label = format!("rustlua-{}-pipeline", key.label());
            pipelines
                .get_or_create(key, || {
                    Ok::<_, std::convert::Infallible>(Arc::new(create_mesh_pipeline(
                        &device,
                        &mesh_pipeline_layout,
                        &shader,
                        target_format,
                        &label,
                        false,
                        rustlua_render::mesh3d::MaterialAlphaMode::Opaque,
                        false,
                    )))
                })
                .expect("3D 网格变体数远低于 family/全局上限");
        }
    }
    for skin in [SkinMode::Off, SkinMode::On] {
        let shader = variant_shader_module(
            &device,
            &SHADOW,
            skin,
            IblMode::Off,
            instance_mode,
            &format!("rustlua-shadow-{}-shader", skin_tag(skin)),
        );
        let key = ShaderVariantKey::shadow(skin);
        let label = format!("rustlua-{}-pipeline", key.label());
        pipelines
            .get_or_create(key, || {
                Ok::<_, std::convert::Infallible>(Arc::new(create_shadow_pipeline(
                    &device,
                    &shadow_pipeline_layout,
                    &shader,
                    &label,
                    false,
                )))
            })
            .expect("阴影变体数远低于 family/全局上限");
    }
    // P3-R04-4：预编译发生在启动期（构造 `Renderer` 时），统计随资源快照暴露给 Profiler。
    // 超预算只告警一次：此刻管线已经建好，取消它们只会把同样的卡顿挪到首帧。
    let variant_stats = pipelines.stats();
    if variant_stats.within_budget() {
        tracing::info!(
            target: "game",
            "[render] shader variants: count={} compile_ms={:.2} hits={} misses={} budget_ms={:.0}",
            variant_stats.variants,
            variant_stats.compile_ms(),
            variant_stats.hits,
            variant_stats.misses,
            VARIANT_COMPILE_BUDGET_MS
        );
    } else {
        tracing::warn!(
            target: "render",
            "[runtime] shader 变体预编译超出预算：count={} compile_ms={:.2} budget_ms={:.0}；请缩减预编译清单，不要每帧补编译",
            variant_stats.variants,
            variant_stats.compile_ms(),
            VARIANT_COMPILE_BUDGET_MS
        );
    }
    Variants {
        instance_mode,
        pipelines,
        hdr_format,
        stats: variant_stats,
    }
}
