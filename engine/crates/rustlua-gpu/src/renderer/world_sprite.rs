//! 世界 Sprite 通道（WP12 阶段 4c 从 `renderer/mod.rs` 原样搬出）：本帧用哪个视图、哪个
//! 深度附件，以及它的顶点缓冲初始容量。类型与自由函数在这一半，`impl Renderer` 的方法在文件
//! 后半（同 crate 的兄弟模块允许同一个类型的另一个固有 `impl` 块）。
//!
//! `Renderer` 结构与全部字段仍留在 `renderer/mod.rs`（计划 §3.1.2）；这里被父模块调用的私有项
//! 按编译器要求放宽为 `pub(super)`（§3.1.4）。

use std::collections::HashMap;

use rustlua_assets::asset_handle::AssetHandle;
use rustlua_camera::camera::{CameraView, RenderView, Viewport};
use rustlua_math::math3d::{Aabb3, Mat4, Vec3};
use rustlua_render::frame::{Scene3d, SceneMesh};
use rustlua_render::instance_batch::InstanceBatch3D;
use rustlua_render::mesh3d::MaterialAlphaMode;
use rustlua_render::water::WaterSpec;

use super::{
    DepthPass, FrameDraw, FrameGpuMotion, GpuMaterial, RenderFrameMetrics, Renderer, StaticGroup,
    apply_declared_viewport, max_model_scale, record_dropped_draw, record_set_pass,
    record_vertex_draw, scaled_model,
};
use rustlua_render::frame::WorldSpriteVertex;

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
        let mut uniform = [0.0_f32; 20];
        uniform[..16].copy_from_slice(&view.matrix.columns.concat());
        uniform[16] = self.clock_secs();
        uniform[17] = self.cloud_coverage.clamp(0.0, 1.0);
        uniform[18] = self.cloud_speed.clamp(0.0, 4.0);
        uniform[19] = self.cloud_shadow_strength.clamp(0.0, 0.6);
        let uniform = bytemuck::cast_slice(&uniform);
        self.queue
            .write_buffer(&self.world_sprite_camera_buffer, 0, uniform);
        // 共用视图时把同一份矩阵也记进宿主字段：诊断读到的永远是"这一帧真正用的那一份"。
        self.world_sprite_view_projection = view.matrix;
        Some(view)
    }

    /// 按 [`SpritePassFilter`] 画世界 Sprite 分段。
    ///
    /// `hdr` 选择目标格式对应的管线：HDR 帧的附件是 `Rgba16Float`，绝不能绑 LDR 管线
    /// （管线与附件格式不符是校验错误，不是"颜色偏一点"）。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_world_sprites(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target: (u32, u32),
        world: WorldSpriteView,
        pass_kind: DepthPass,
        hdr: bool,
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
        let pipeline = if hdr {
            match pass_kind {
                DepthPass::WorldSpriteOpaque => self.hdr_world_sprite_opaque_pipeline.as_ref(),
                _ => self.hdr_world_sprite_pipeline.as_ref(),
            }
        } else {
            Some(match pass_kind {
                DepthPass::WorldSpriteOpaque => &self.world_sprite_opaque_pipeline,
                _ => &self.world_sprite_pipeline,
            })
        };
        let Some(pipeline) = pipeline else {
            // HDR 激活条件里已经要求两条管线都存在；走到这里说明构造期与运行期分叉了。
            // 不静默空转：记 dropped 并告警，通道不会出现在 `last_frame_passes` 里，计划对账可查。
            record_dropped_draw(metrics);
            tracing::warn!(
                target: "render",
                hdr,
                "[runtime] world sprite pass skipped: pipeline unavailable"
            );
            return;
        };
        if world.shared_depth {
            self.ssao_depth_view = self.depth_view.clone();
        } else {
            self.ssao_depth_view = self.world_sprite_depth_view.clone();
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
        pass.set_pipeline(pipeline);
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
        // P3-R183：剔除计数每帧重置；候选 = 网格已缓存、进入剔除判定的 drawable×primitive。
        // 用局部计数：`cached_mesh` 的借用横跨整个循环，期间不能写 `self`。
        let mut cull_candidates = 0u64;
        let mut cull_frustum = 0u64;
        // P3-R206 读数补全：距离档的剔除量单独记一份，用来在不拆分母的前提下归因到档位。
        let mut cull_distance = 0u64;
        let mut skip_mesh = 0u64;
        // 剔除视锥来自本帧**视图的烘焙结果**（拆分批 7）：与写进 uniform 的矩阵同源，
        // 因此"画出来的"与"被剔除的"不可能按两个比例各算一套。
        let frustum = if self.cull_enabled {
            scene.view.baked().map(CameraView::frustum)
        } else {
            None
        };
        // 键借用 `scene` 里的路径，**每帧不再为每个 drawable 克隆两个 `String`**：
        // 10,000 个静态网格的 WP0 阶梯里，这一处曾是每帧 2 万次堆分配（docs/53 的
        // "每帧分配"口径），而键只需要在本次调用内有效。`scene` 的生存期覆盖整个函数，
        // `self` 只被 `&self` 读取（网格缓存/缩放查询），因此借用键不会和 `&mut self` 冲突。
        let mut groups: HashMap<(&str, usize, Option<&str>), StaticGroup> = HashMap::new();
        let mut draws: Vec<FrameDraw> = Vec::new();
        // P3-R02-4：半透明单独排队，最后按到相机的距离**从远到近**画；不参与实例合批
        // （合批会打乱逐实例的透明顺序）。
        let mut blend_draws: Vec<FrameDraw> = Vec::new();
        let camera_position = scene.camera.position;
        let instancing_enabled = self.instancing_enabled;
        // P3-R245：本帧暂存的运动批次按实体认领；没被认领的直接丢弃（不跨帧残留）。
        // 档位关 / 没有 compute 能力时**连取走都不做**（`stage_motion_batch` 的输入原样留到
        // 下一帧再被丢弃），默认路径一个字节都不碰。
        let gpu_motion_enabled = self.gpu_motion_batches && self.compute_enabled();
        let staged_motion = if gpu_motion_enabled {
            std::mem::take(&mut self.staged_motion)
        } else {
            Vec::new()
        };
        // `cached_mesh` 取 `&mut self`，档位必须先拷出来再用（同 `instancing_enabled`）。
        let max_draw_distance = self.max_draw_distance;
        // P3-R206 分类档：同一个借用原因，子串过滤也先拷出来。
        let max_draw_distance_filter = self.max_draw_distance_filter.clone();
        // P3-R248：阴影投射排除档同理（每帧一次 `Option<String>` 克隆，与上一条同价）。
        let shadow_caster_skip = self.shadow_caster_skip.clone();
        // P3-R265：点光链专用排除档（同一个借用原因）。
        let point_shadow_caster_skip = self.point_shadow_caster_skip.clone();
        // P3-R252：整批跳过的球心/半径与烘焙侧同一份口径（`update_3d_uniforms` 每帧先写）。
        // 借用原因同 `max_draw_distance`：`cached_mesh` 要 `&mut self`。
        let batch_cull = (self.gpu_motion_cull_distance > 0.0)
            .then_some((self.camera_world_position, self.gpu_motion_cull_distance));
        let mut gpu_motion_culled = 0u64;
        let mut gpu_motion_culled_instances = 0u64;
        // 覆盖材质先统一解析：失败会缓存为 `None` 并告警一次，绘制侧回退内嵌材质。
        // 预解析还避免在遍历 `scene.drawables`（不可变借用）时对 `self` 取可变借用。
        let mut overrides: HashMap<&str, GpuMaterial> = HashMap::new();
        for drawable in &scene.drawables {
            if let Some(path) = drawable.material.as_deref()
                && !overrides.contains_key(path)
                && let Some(material) = self.material_override(path)
            {
                overrides.insert(path, material);
            }
        }
        for drawable in &scene.drawables {
            // P3-R184：归因开关在缓存查询与剔除**之前**跳过 ⇒ 不进候选分母，
            // 两档 A/B 的剔除率口径不受开关本身污染。
            if let Some(filter) = &self.skip_mesh_filter
                && drawable.mesh_path.contains(filter.as_str())
            {
                skip_mesh += 1;
                continue;
            }
            // 导入设置里的"模型缩放"在这一步乘进模型矩阵（**在剔除之前**）：
            // 剔除球用同一份矩阵算，缩放后的模型才不会被按旧尺寸剔掉。
            let scale = self.model_scale_for(&drawable.mesh_path);
            // P3-R248：方向光 CSM 的分类排除档。样本名与网格路径都算类别（无实例合批时
            // 逐 drawable 判；静态合批组只有一个路径，见下面的组口径）。
            let drawable_skips_shadow = shadow_caster_skip.as_deref().is_some_and(|filter| {
                drawable.mesh_path.contains(filter) || drawable.name.contains(filter)
            });
            // P3-R265：点光专用档用同一套"路径或样本名"口径；两档叠加时点光链取并集。
            let drawable_skips_point_shadow =
                point_shadow_caster_skip.as_deref().is_some_and(|filter| {
                    drawable.mesh_path.contains(filter) || drawable.name.contains(filter)
                });
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
                // P3-R245：本帧暂存了这批实例的运动输入 ⇒ 位姿交给 compute 求值。
                // **放弃逐实例剔除与合批**（R244 模块文档：GPU 批必须单独一条 draw，实例数
                // 口径才对得上）；包围盒仍按同一批 CPU 矩阵并集算，阴影 range 剔除的结果与
                // CPU 档一致。`models` 留作烘焙失败时的回退。
                if let Some((_, batch, time)) = staged_motion
                    .iter()
                    .find(|(entity, _, _)| *entity == drawable.entity.0)
                    && !batch.instances.is_empty()
                {
                    cull_candidates += batch.instances.len() as u64;
                    // P3-R246：宿主跳过 CPU 矩阵时 `drawable.instances` 为空——实例数照样按
                    // 批次算，包围盒用"路径点 AABB 外扩最大实例半径"（保守，不误剔）。
                    let bounds = if drawable.instances.is_empty() {
                        conservative_batch_bounds(batch, primitive.bounds.1, scale)
                    } else {
                        let mut bounds = Aabb3 {
                            min: Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY),
                            max: Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY),
                        };
                        for &instance_model in &drawable.instances {
                            let model = scaled_model(instance_model, scale);
                            let center = model
                                .transform_point(primitive.bounds.0)
                                .unwrap_or(primitive.bounds.0);
                            let radius = primitive.bounds.1 * max_model_scale(model);
                            bounds.min = Vec3::new(
                                bounds.min.x.min(center.x - radius),
                                bounds.min.y.min(center.y - radius),
                                bounds.min.z.min(center.z - radius),
                            );
                            bounds.max = Vec3::new(
                                bounds.max.x.max(center.x + radius),
                                bounds.max.y.max(center.y + radius),
                                bounds.max.z.max(center.z + radius),
                            );
                        }
                        bounds
                    };
                    // P3-R252：整批都在剔除球外 ⇒ bake 会把每一只都写成零矩阵（退化、不画）。
                    // 这条 draw 干脆不登记：像素完全相同，但阴影的 53 个 pass、planar 与主视图
                    // 都不再各录一遍它。判据用上面那份**保守**包围盒（只会少剔，不会误剔）。
                    if let Some((origin, radius)) = batch_cull
                        && bounds_outside_sphere(bounds, origin, radius)
                    {
                        gpu_motion_culled += 1;
                        gpu_motion_culled_instances += batch.instances.len() as u64;
                        continue;
                    }
                    let mut material = primitive.material;
                    material.skinned = false;
                    if let Some(path) = drawable.material.as_deref()
                        && let Some(overridden) = overrides.get(path)
                    {
                        material = *overridden;
                        material.skinned = false;
                    }
                    let draw = FrameDraw {
                        geometry: primitive.geometry.clone(),
                        material,
                        palette: Vec::new(),
                        // P3-R246：位姿只走 GPU——`models` 不再克隆一遍 CPU 矩阵（宿主跳过
                        // 求值后它本来就是空的）；烘焙失败时在绑定组建制处按 `matrices_at` 补算。
                        models: Vec::new(),
                        water: WaterSpec::is_water_path(&drawable.mesh_path),
                        bounds,
                        gpu_motion: Some(FrameGpuMotion {
                            batch: batch.clone(),
                            time: *time,
                            model_scale: scale,
                        }),
                        // P3-R255：局部包围球从原点量起（球心偏移 + 半径），逐实例再乘缩放。
                        gpu_motion_local_radius: primitive.bounds.0.length() + primitive.bounds.1,
                        cast_shadow: !drawable_skips_shadow,
                        cast_point_shadow: !drawable_skips_shadow && !drawable_skips_point_shadow,
                    };
                    if material.alpha_mode == MaterialAlphaMode::Blend {
                        blend_draws.push(draw);
                    } else {
                        draws.push(draw);
                    }
                    continue;
                }
                // P3-R237：实例批次（crowd）——一个 drawable 带 N 份实例矩阵时，剔除与合批
                // 都**逐实例**进行：一只被剔掉的蚂蚁不影响同批的其它蚂蚁，留在视锥里的
                // 那些合进同一次实例化绘制。候选计数随之按**实例**计（普通 drawable 仍是
                // 每 primitive 一份）。
                let single_model = [drawable.model];
                let models: &[Mat4] = if drawable.instances.is_empty() {
                    &single_model
                } else {
                    &drawable.instances
                };
                cull_candidates += models.len() as u64;
                for &instance_model in models {
                    let model = scaled_model(instance_model, scale);
                    // 剔除与 P3-R188 的阴影 range 剔除共用同一份世界包围球：剔除关掉也要算。
                    let world_center = model
                        .transform_point(primitive.bounds.0)
                        .unwrap_or(primitive.bounds.0);
                    let world_radius = primitive.bounds.1 * max_model_scale(model);
                    if let Some(frustum) = frustum
                        && !frustum.contains_sphere(world_center, world_radius)
                    {
                        cull_frustum += 1;
                        continue;
                    }
                    // P3-R206：最大绘制距离（0 = 关闭）。按包围球边缘判：近处大物体的可见
                    // 边缘不能因为"圆心在远处"被整块剔掉。
                    if max_draw_distance > 0.0
                        && max_draw_distance_filter
                            .as_deref()
                            .is_none_or(|filter| drawable.mesh_path.contains(filter))
                        && (world_center - camera_position).length()
                            > max_draw_distance + world_radius
                    {
                        cull_frustum += 1;
                        cull_distance += 1;
                        continue;
                    }
                    let bounds = Aabb3 {
                        min: Vec3::new(
                            world_center.x - world_radius,
                            world_center.y - world_radius,
                            world_center.z - world_radius,
                        ),
                        max: Vec3::new(
                            world_center.x + world_radius,
                            world_center.y + world_radius,
                            world_center.z + world_radius,
                        ),
                    };
                    let skinned = !drawable.palette.is_empty();
                    let mut material = primitive.material;
                    material.skinned = skinned;
                    if let Some(path) = drawable.material.as_deref()
                        && let Some(overridden) = overrides.get(path)
                    {
                        material = *overridden;
                        material.skinned = skinned;
                    }
                    if !skinned
                        && instancing_enabled
                        && material.alpha_mode != MaterialAlphaMode::Blend
                    {
                        let group = groups
                            .entry((
                                drawable.mesh_path.as_str(),
                                prim_index,
                                drawable.material.as_deref(),
                            ))
                            .or_insert_with(|| StaticGroup {
                                geometry: primitive.geometry.clone(),
                                material,
                                models: Vec::new(),
                                water: WaterSpec::is_water_path(&drawable.mesh_path),
                                bounds,
                                cast_shadow: !shadow_caster_skip
                                    .as_deref()
                                    .is_some_and(|filter| drawable.mesh_path.contains(filter)),
                                // P3-R265：组口径同样只按路径判（组键里只有路径）。
                                cast_point_shadow: !shadow_caster_skip
                                    .as_deref()
                                    .is_some_and(|filter| drawable.mesh_path.contains(filter))
                                    && !point_shadow_caster_skip
                                        .as_deref()
                                        .is_some_and(|filter| drawable.mesh_path.contains(filter)),
                            });
                        group.models.push(model);
                        group.bounds.min = Vec3::new(
                            group.bounds.min.x.min(bounds.min.x),
                            group.bounds.min.y.min(bounds.min.y),
                            group.bounds.min.z.min(bounds.min.z),
                        );
                        group.bounds.max = Vec3::new(
                            group.bounds.max.x.max(bounds.max.x),
                            group.bounds.max.y.max(bounds.max.y),
                            group.bounds.max.z.max(bounds.max.z),
                        );
                    } else if material.alpha_mode == MaterialAlphaMode::Blend {
                        blend_draws.push(FrameDraw {
                            geometry: primitive.geometry.clone(),
                            material,
                            palette: drawable.palette.clone(),
                            models: vec![model],
                            water: WaterSpec::is_water_path(&drawable.mesh_path),
                            bounds,
                            gpu_motion: None,
                            gpu_motion_local_radius: 0.0,
                            cast_shadow: !drawable_skips_shadow,
                            cast_point_shadow: !drawable_skips_shadow
                                && !drawable_skips_point_shadow,
                        });
                    } else {
                        draws.push(FrameDraw {
                            geometry: primitive.geometry.clone(),
                            material,
                            palette: drawable.palette.clone(),
                            models: vec![model],
                            water: WaterSpec::is_water_path(&drawable.mesh_path),
                            bounds,
                            gpu_motion: None,
                            gpu_motion_local_radius: 0.0,
                            cast_shadow: !drawable_skips_shadow,
                            cast_point_shadow: !drawable_skips_shadow
                                && !drawable_skips_point_shadow,
                        });
                    }
                }
            }
        }
        let mut largest_groups: Vec<(usize, String)> = groups
            .iter()
            .map(|((path, primitive, material), group)| {
                (
                    group.models.len(),
                    format!("{}#{}@{}", path, primitive, material.unwrap_or("<default>")),
                )
            })
            .collect();
        largest_groups.sort_by_key(|group| std::cmp::Reverse(group.0));
        largest_groups.truncate(6);
        let instanced_group_count = groups.len();
        for group in groups.into_values() {
            draws.push(FrameDraw {
                geometry: group.geometry,
                material: group.material,
                palette: Vec::new(),
                models: group.models,
                water: group.water,
                bounds: group.bounds,
                gpu_motion: None,
                gpu_motion_local_radius: 0.0,
                cast_shadow: group.cast_shadow,
                cast_point_shadow: group.cast_point_shadow,
            });
        }
        // P3-R185：不透明前到后排序——近处网格先写深度，远处被挡像素（被草/蚁压住的
        // 地形）被 early-Z 拒绝，不再白跑一遍 PBR 片元。不透明渲染与顺序无关，两档像素
        // 一致；键取组内最近实例的平方距离（整组近者先画，组内远实例仍靠深度互拒）。
        // 稳定排序：等距保留声明顺序。Blend 队列不参与，仍按远→近排在最后。
        if self.depth_sort_enabled {
            // P3-R246：GPU 位姿批次的 `models` 可能为空 ⇒ 用保守包围盒中心作排序键
            // （排序只为 early-Z，键的精度不影响像素）。
            let nearest_distance = |draw: &FrameDraw| {
                if draw.models.is_empty() {
                    let center = draw_center(draw.bounds);
                    let dx = center.x - camera_position.x;
                    let dy = center.y - camera_position.y;
                    let dz = center.z - camera_position.z;
                    return dx * dx + dy * dy + dz * dz;
                }
                draw.models
                    .iter()
                    .map(|model| {
                        let dx = model.columns[3][0] - camera_position.x;
                        let dy = model.columns[3][1] - camera_position.y;
                        let dz = model.columns[3][2] - camera_position.z;
                        dx * dx + dy * dy + dz * dz
                    })
                    .fold(f32::INFINITY, f32::min)
            };
            draws.sort_by(|left, right| {
                nearest_distance(left)
                    .partial_cmp(&nearest_distance(right))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        // 画家算法：远 → 近。距离取实例平移分量到相机的平方距离；排序是稳定的，
        // 等距时保留场景声明顺序（golden 可复现）。
        blend_draws.sort_by(|left, right| {
            let distance = |draw: &FrameDraw| {
                // P3-R246：空 `models`（GPU 位姿批次）不能按下标取，用包围盒中心。
                let center = draw.models.first().map_or_else(
                    || draw_center(draw.bounds),
                    |model| {
                        Vec3::new(
                            model.columns[3][0],
                            model.columns[3][1],
                            model.columns[3][2],
                        )
                    },
                );
                let dx = center.x - camera_position.x;
                let dy = center.y - camera_position.y;
                let dz = center.z - camera_position.z;
                dx * dx + dy * dy + dz * dz
            };
            distance(right)
                .partial_cmp(&distance(left))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        draws.extend(blend_draws);
        self.frame_cull_candidates = cull_candidates;
        self.frame_cull_frustum = cull_frustum;
        self.frame_cull_distance = cull_distance;
        self.frame_skip_mesh = skip_mesh;
        self.frame_gpu_motion_culled = gpu_motion_culled;
        self.frame_gpu_motion_culled_instances = gpu_motion_culled_instances;
        // Periodic M8-08 baseline: instance/draw counts with culling and
        // instancing active, useful for comparing against --no-cull/--no-instance.
        self.frame_count = self.frame_count.wrapping_add(1);
        if self.frame_count.is_multiple_of(120) {
            let instances: usize = draws.iter().map(FrameDraw::instance_count).sum();
            tracing::info!(
                target: "render",
                draws = draws.len(),
                instances,
                instanced_groups = instanced_group_count,
                largest_groups = ?largest_groups,
                cull = self.cull_enabled,
                cull_candidates = self.frame_cull_candidates,
                cull_frustum = self.frame_cull_frustum,
                cull_distance = self.frame_cull_distance,
                skip_mesh = self.frame_skip_mesh,
                gpu_motion_culled = self.frame_gpu_motion_culled,
                gpu_motion_culled_instances = self.frame_gpu_motion_culled_instances,
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

/// 世界包围盒中心（排序键用；空盒退回原点，排序不参与像素决策）。
fn draw_center(bounds: Aabb3) -> Vec3 {
    Vec3::new(
        (bounds.min.x + bounds.max.x) * 0.5,
        (bounds.min.y + bounds.max.y) * 0.5,
        (bounds.min.z + bounds.max.z) * 0.5,
    )
}

/// P3-R246：`drawable.instances` 为空（宿主跳过 CPU 矩阵）时的**保守**世界包围盒。
///
/// 运动实例的世界中心必落在路径上，姿态怎么转也逃不出"局部包围球 × 缩放"；因此
/// 路径节点 AABB 外扩 `局部半径 × 最大实例缩放 × 模型缩放` 就是上界。静态批次按实例
/// 位置取点。路径非法/实例表为空时退回一个巨大的盒（宁可多画，不可误剔阴影）。
fn conservative_batch_bounds(
    batch: &InstanceBatch3D,
    local_radius: f32,
    model_scale: f32,
) -> Aabb3 {
    let mut max_scale = 0.0f32;
    for instance in &batch.instances {
        let scale = instance.scale;
        if scale.iter().all(|value| value.is_finite()) {
            max_scale = max_scale
                .max(scale[0].abs())
                .max(scale[1].abs())
                .max(scale[2].abs());
        }
    }
    let radius = local_radius * max_scale * model_scale;
    let positions = || {
        batch
            .instances
            .iter()
            .map(|instance| instance.position)
            .collect::<Vec<[f32; 3]>>()
    };
    let nodes: Vec<[f32; 3]> = match batch.motion.as_ref() {
        Some(motion) => match motion.arc() {
            Some(arc) => arc.nodes(&motion.path),
            None => positions(),
        },
        None => positions(),
    };
    let mut bounds = Aabb3 {
        min: Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY),
        max: Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY),
    };
    for node in nodes {
        bounds.min = Vec3::new(
            bounds.min.x.min(node[0] - radius),
            bounds.min.y.min(node[1] - radius),
            bounds.min.z.min(node[2] - radius),
        );
        bounds.max = Vec3::new(
            bounds.max.x.max(node[0] + radius),
            bounds.max.y.max(node[1] + radius),
            bounds.max.z.max(node[2] + radius),
        );
    }
    if !bounds.min.x.is_finite() {
        return Aabb3 {
            min: Vec3::new(-1.0e6, -1.0e6, -1.0e6),
            max: Vec3::new(1.0e6, 1.0e6, 1.0e6),
        };
    }
    bounds
}

/// P3-R252：包围盒与剔除球**完全不相交**（球心到盒的最近距离 > 半径）。
///
/// 这是"整批都可剔"的保守判据：盒是实例原点集合的超集 ⇒ 盒外则每个原点都在球外，
/// GPU 会把每只都写成零矩阵。反过来盒与球相交时一律不跳（宁可多录，不许误剔）。
fn bounds_outside_sphere(bounds: Aabb3, origin: Vec3, radius: f32) -> bool {
    let nearest = Vec3::new(
        origin.x.clamp(bounds.min.x, bounds.max.x),
        origin.y.clamp(bounds.min.y, bounds.max.y),
        origin.z.clamp(bounds.min.z, bounds.max.z),
    );
    let dx = nearest.x - origin.x;
    let dy = nearest.y - origin.y;
    let dz = nearest.z - origin.z;
    dx * dx + dy * dy + dz * dz > radius * radius
}
