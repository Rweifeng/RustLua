//! 世界 Sprite 通道（WP12 阶段 4c 从 `renderer/mod.rs` 原样搬出）：本帧用哪个视图、哪个
//! 深度附件，以及它的顶点缓冲初始容量。类型与自由函数在这一半，`impl Renderer` 的方法在文件
//! 后半（同 crate 的兄弟模块允许同一个类型的另一个固有 `impl` 块）。
//!
//! `Renderer` 结构与全部字段仍留在 `renderer/mod.rs`（计划 §3.1.2）；这里被父模块调用的私有项
//! 按编译器要求放宽为 `pub(super)`（§3.1.4）。

use std::collections::HashMap;

use crate::host::frame::{Scene3d, SceneMesh};
use rustlua_assets::asset_handle::AssetHandle;
use rustlua_camera::camera::{CameraView, RenderView, Viewport};
use rustlua_math::math3d::Mat4;

use super::{
    DepthPass, FrameDraw, GpuMaterial, RenderFrameMetrics, Renderer, StaticGroup,
    apply_declared_viewport, max_model_scale, record_dropped_draw, record_set_pass,
    record_vertex_draw,
};
use crate::host::frame::WorldSpriteVertex;

/// 世界 Sprite 通道本帧用哪个视图、哪个深度附件（docs/54 §8-④）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WorldSpriteView {
    /// 世界 → 裁剪空间矩阵：共用视图时与 3D 网格**同一份**（来自宿主声明的 `RenderView`）。
    pub(super) matrix: Mat4,
    /// 声明的视口；`None` = 沿用渲染器默认的 overlay 视口（宿主未声明视图的回滚档）。
    pub(super) viewport: Option<Viewport>,
    /// 是否与 3D 网格共用深度附件。共用时精灵与网格互相遮挡；不共用时只有精灵之间的顺序。
    pub(super) shared_depth: bool,
}

/// 一次世界 Sprite 绘制要挑哪些分段。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SpritePassFilter {
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
pub(super) fn resolve_world_sprite_view(
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

/// 世界 Sprite 顶点缓冲初始容量（按需扩容）。4096 个顶点 ≈ 每帧 1000 多个精灵；
/// 超限时扩容逻辑与 2D overlay 缓冲同思路（2 的幂次增长）。
pub(super) const WORLD_SPRITE_BUFFER_BYTES: u64 =
    4096 * std::mem::size_of::<WorldSpriteVertex>() as u64;

impl Renderer {
    /// 决定本帧世界 Sprite 用哪个视图/深度附件，并把顶点与相机 uniform 准备好。
    ///
    /// 视图来源优先级（docs/54 §8-③/④）：
    /// 1. 宿主声明了主视图 ⇒ 与主视图**共用**相机、视口与深度附件（混合 2D/3D 的正确形态）；
    /// 2. 只有 overlay（纯 2D 画面）⇒ 用 overlay 的世界相机，深度附件独立；
    /// 3. 什么都没声明 ⇒ 用宿主显式设置的矩阵与默认 overlay 视口（收敛前的回滚档）。
    pub(super) fn prepare_world_sprites(
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
    pub(super) fn draw_world_sprites(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target: (u32, u32),
        world: WorldSpriteView,
        pass_kind: DepthPass,
        draws: &[rustlua_render::sprite_batch::SpriteDrawBatch],
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
        record_set_pass(metrics);
        pass.set_bind_group(0, &self.world_sprite_camera_bind_group, &[]);
        pass.set_vertex_buffer(0, self.world_sprite_buffer.slice(..));
        apply_declared_viewport(&mut pass, world.viewport, target);
        for draw in selected {
            let texture = draw.texture_id + 1;
            let Some(bind) = self.bind_group_for(texture, paths, handles) else {
                // 与 2D 叠加通道同一条口径（`passes.rs`）：**画不出来必须留痕**。
                // 静默 `continue` 的症状是"世界里少一个 sprite、日志一个字都没有"。
                record_dropped_draw(metrics);
                tracing::warn!(
                    target: "render",
                    texture,
                    vertex_start = draw.vertex_start,
                    vertex_count = draw.vertex_count,
                    "[runtime] world sprite draw dropped: bind group unavailable"
                );
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
    pub(super) fn frame_3d_draws(&mut self, scene: &Scene3d) -> Vec<FrameDraw> {
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
                // 这里**刻意只记 debug、不计入 `dropped_draws`**：网格不在缓存绝大多数是
                // **异步预热的头几帧**（下一帧就有），把它算成"丢了 draw"会让正常帧也报非零，
                // 反而毁掉那个计数"0 = 这一帧什么都没丢"的含义。真正的**硬失败**（加载失败）
                // 已在加载侧留痕（`gpu_mesh.rs` 的 `async asset load failed` 告警）。
                tracing::debug!(
                    target: "render",
                    path = %drawable.mesh_path,
                    "[render] 3D mesh not in cache yet; skipping this frame"
                );
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

    /// WP6：写入世界 Sprite 通道用的投影矩阵——**只在宿主没有声明任何 [`rustlua_camera::camera::FrameViews`] 时**
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
}
