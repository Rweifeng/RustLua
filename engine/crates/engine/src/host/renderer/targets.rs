//! `Renderer` 的渲染目标与附件视图（WP12 阶段 4a 从 `renderer/mod.rs` 原样搬出）：
//! 深度视图、阴影视图、`resize`、深度/后处理场景视图的按需重建。
//!
//! 结构体仍定义在 `renderer/mod.rs`——字段是私有的，搬走结构体会逼着几十条字段放宽可见性；
//! 固有 `impl` 块不要求和类型同模块（同 crate 即可），而这里的私有方法要能被父模块的
//! `new_async`/`render_passes` 调用 ⇒ 按编译器指出的那几条加 `pub(super)`（计划 §3.1.4）。

use crate::core::window::has_renderable_size;
use wgpu::Device;
use winit::dpi::PhysicalSize;

use super::Renderer;

impl Renderer {
    pub(super) fn create_depth_view(device: &Device, width: u32, height: u32) -> wgpu::TextureView {
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

    pub(super) fn create_shadow_view(device: &Device, size: u32) -> wgpu::TextureView {
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
    pub(super) fn ensure_depth_view(&mut self, width: u32, height: u32) {
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
    pub(super) fn ensure_post_process_scene_view(&mut self, width: u32, height: u32) {
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
}
