//! 渲染通道的执行侧（WP12 阶段 4c 从 `renderer/mod.rs` 原样搬出）：`render`、
//! `render_into_texture`、后处理通道 `draw_post_process`，以及 446 行的 `render_passes`。
//!
//! 计划 §5 的硬规矩 2：**只搬位置，不在本工作包内拆函数体** —— `render_passes` 按原样整体搬，
//! 拆成 `passes/clear.rs` / `passes/world2d.rs` 之类属于改变结构，另开工作包（§9）。
//! `Renderer` 结构体与字段仍留在 `renderer/mod.rs`；被父模块调用的私有方法按编译器要求放宽。

use crate::host::frame::{Scene3d, SceneMesh};
use rustlua_camera::camera::{ClearPolicy, DEFAULT_3D_CLEAR, RenderView};
use rustlua_infra::logging::{EngineError, LogModule};
use rustlua_render::layer::{RenderLayer, layer_indices};
use rustlua_render::post_process::PostProcess;
use rustlua_render::render_plan::{FrameFacts, RenderPass, RenderPlan};
// M73-00 WP5：时钟必须走 `web-time` 而不是 `std::time`。`wasm32-unknown-unknown` 的 std
// 落在 `sys/pal/unsupported` 上，`Instant::now()` 会**直接 panic**（"time not implemented on
// this platform"，见 std 源码 `sys/time/unsupported.rs`）——渲染器是第一个真在 wasm 上被调用的
// 引擎子系统，帧计时（`cpu_frame_us`）就会踩到它。`web-time` 在原生上**就是** `std::time::Instant`
// 的再导出（类型相同、行为为零变化），在 wasm 上换成 `performance.now()`。
use web_time::Instant;
use wgpu::SurfaceError;

use super::world_sprite::SpritePassFilter;

use super::{
    DepthPass, RenderFrameMetrics, RenderOutcome, Renderer, apply_declared_viewport,
    count_layer_draws, hash_bytes, record_dropped_draw, record_indexed_draw, record_triangle_draw,
    record_vertex_draw,
};

impl Renderer {
    /// 执行后处理通道：把离屏场景颜色按曝光/色调/暗角写进目标附件。
    fn draw_post_process(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
        settings: PostProcess,
        target_size: (u32, u32),
    ) -> u64 {
        self.queue.write_buffer(
            &self.post_process_params,
            0,
            bytemuck::bytes_of(&settings.uniform()),
        );
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-post-process-bind-group"),
            layout: &self.post_process_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.post_process_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.post_process_params.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rustlua-post-process-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // 全屏三角形覆盖整个附件，但 Load 会保留附件上原有内容；显式 Clear 让结果
                    // 与"上一帧残留"无关（同一张附件被复用时才看得出来）。
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.post_process_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        // 整目标：场景离屏纹理与目标同尺寸，而场景本身的视口只影响它写进离屏的那部分。
        apply_declared_viewport(&mut pass, None, target_size);
        pass.draw(0..3, 0..1);
        3
    }

    pub fn render(&mut self, mesh: &SceneMesh, scene3d: Option<&Scene3d>) -> RenderOutcome {
        let Some(surface) = &self.surface else {
            tracing::warn!(target: "game", "[render] render() requires a window surface; use render_into_texture");
            return RenderOutcome::Skipped;
        };
        let output = match surface.get_current_texture() {
            Ok(output) => output,
            Err(SurfaceError::Lost | SurfaceError::Outdated) => {
                surface.configure(&self.device, &self.config);
                tracing::info!(target: "game", "[render] surface reconfigured after transient error");
                return RenderOutcome::Retry;
            }
            Err(SurfaceError::Timeout) => {
                tracing::info!(target: "game", "[render] surface acquire timed out; skipping frame");
                return RenderOutcome::Skipped;
            }
            Err(SurfaceError::OutOfMemory) => {
                tracing::error!(target: "engine", error = %EngineError::new(LogModule::Engine, "render frame", "GPU is out of memory").with_suggestion("close other GPU applications or reduce resource usage"), "GPU out of memory; requesting shutdown");
                return RenderOutcome::Fatal;
            }
            Err(SurfaceError::Other) => {
                tracing::info!(target: "game", "[render] surface acquire failed with an unknown error");
                return RenderOutcome::Skipped;
            }
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-clear-encoder"),
            });
        let started = Instant::now();
        let completed_gpu_us = self.poll_gpu_timestamp();
        let timestamp_slot = self.acquire_timestamp_slot();
        if timestamp_slot.is_some()
            && let Some(query_set) = &self.timestamp_query_set
        {
            encoder.write_timestamp(query_set, 0);
        }
        let mut metrics = self.render_passes(
            &mut encoder,
            mesh,
            scene3d,
            &view,
            (self.config.width, self.config.height),
        );
        if let Some(slot) = timestamp_slot {
            self.finish_gpu_timestamp(&mut encoder, slot);
        }
        let submission = self.queue.submit([encoder.finish()]);
        if let Some(slot) = timestamp_slot {
            self.arm_timestamp_slot(slot, submission);
        }
        metrics.cpu_frame_us = started.elapsed().as_micros() as u64;
        metrics.gpu_frame_us = completed_gpu_us;
        self.last_frame_metrics = metrics;
        output.present();
        RenderOutcome::Presented
    }

    /// Renders the current frame into an offscreen texture view (no present).
    /// The caller owns the target texture and supplies its view + dimensions;
    /// the depth attachment is rebuilt to match the target size.
    pub fn render_into_texture(
        &mut self,
        mesh: &SceneMesh,
        scene3d: Option<&Scene3d>,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> RenderOutcome {
        self.ensure_depth_view(width, height);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-offscreen-encoder"),
            });
        let started = Instant::now();
        let completed_gpu_us = self.poll_gpu_timestamp();
        let timestamp_slot = self.acquire_timestamp_slot();
        if timestamp_slot.is_some()
            && let Some(query_set) = &self.timestamp_query_set
        {
            encoder.write_timestamp(query_set, 0);
        }
        let mut metrics = self.render_passes(&mut encoder, mesh, scene3d, view, (width, height));
        if let Some(slot) = timestamp_slot {
            self.finish_gpu_timestamp(&mut encoder, slot);
        }
        let submission = self.queue.submit([encoder.finish()]);
        if let Some(slot) = timestamp_slot {
            self.arm_timestamp_slot(slot, submission);
        }
        metrics.cpu_frame_us = started.elapsed().as_micros() as u64;
        metrics.gpu_frame_us = completed_gpu_us;
        self.last_frame_metrics = metrics;
        RenderOutcome::Presented
    }

    /// Shared pass rendering into a given color view (window surface or
    /// offscreen target): clears the view, draws 3D (shadow + mesh into the
    /// depth view), then the 2D overlay.
    ///
    /// WP5：清屏与视口不再由本函数写死——它们来自宿主声明的 [`rustlua_camera::camera::FrameViews`]。
    fn render_passes(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        mesh: &SceneMesh,
        scene3d: Option<&Scene3d>,
        view: &wgpu::TextureView,
        target: (u32, u32),
    ) -> RenderFrameMetrics {
        // 清屏：宿主声明了主视图就按声明走（`ClearPolicy::None` = 不清，叠在已有内容上）；
        // 没声明则保持收敛前的历史默认色，因此"不调用 set_frame_views"像素完全不变。
        let clear = match self.frame_views.clear() {
            Some(ClearPolicy::Color(color)) => Some(wgpu::Color {
                r: f64::from(color[0]),
                g: f64::from(color[1]),
                b: f64::from(color[2]),
                a: f64::from(color[3]),
            }),
            Some(ClearPolicy::None) => None,
            None => Some(wgpu::Color {
                r: f64::from(DEFAULT_3D_CLEAR[0]),
                g: f64::from(DEFAULT_3D_CLEAR[1]),
                b: f64::from(DEFAULT_3D_CLEAR[2]),
                a: f64::from(DEFAULT_3D_CLEAR[3]),
            }),
        };
        let main_viewport = self.frame_views.main.map(|declared| declared.viewport);
        let overlay_viewport = self.frame_views.overlay.map(|declared| declared.viewport);
        // docs/54 §4.3：打开后处理后场景先画进**离屏**附件，那张附件上没有"上一帧留下的内容"
        // 可叠，因此 `ClearPolicy::None` 无法照原义执行。这里显式改成历史默认清屏色并告警一次
        // ——静默把 `None` 当 `Color(默认)` 会让"我声明了不清屏却没生效"变成无从查起的问题。
        let clear = match clear {
            None if self.post_process.is_some() => {
                tracing::warn!(
                    target: "render",
                    "[runtime] post-process需要确定的基色：本帧的 ClearPolicy::None 按默认清屏色执行"
                );
                Some(wgpu::Color {
                    r: f64::from(DEFAULT_3D_CLEAR[0]),
                    g: f64::from(DEFAULT_3D_CLEAR[1]),
                    b: f64::from(DEFAULT_3D_CLEAR[2]),
                    a: f64::from(DEFAULT_3D_CLEAR[3]),
                })
            }
            clear => clear,
        };
        // docs/54 §8-③：本帧世界视图的投影矩阵**只有一份来源**——宿主声明的 `RenderView`。
        // 渲染器不再自己回查 `Camera3D`（那只是"宿主没声明"时的历史回退），回退吃的是
        // 场景提取时就已按视口烘焙好的 `Scene3d.view`（拆分批 7）。
        let main_matrix = self
            .frame_views
            .main
            .and_then(RenderView::matrix)
            .or_else(|| scene3d.and_then(|scene| scene.view.matrix()));
        // 设备没有存储缓冲时（WebGL2 就是这样），3D 网格/阴影这两条路在构造期就没建出真管线
        // （见 `construct.rs` 的 `storage_capable`）。这里**不取帧内 3D draw**：`FrameFacts` 随之
        // 为假，通道计划里就不会有 `Shadow`/`DepthOpaque`，因此那条占位管线不可能被绑上。
        // 代价必须可见：丢弃的 draw 记进 `metrics.dropped_draws`（构造期另有一条告警）。
        let mut dropped_3d_draws = 0u64;
        let frame_draws = if let Some(scene) = scene3d {
            if self.storage_capable {
                self.update_3d_uniforms(scene, main_matrix);
                self.frame_3d_draws(scene)
            } else {
                dropped_3d_draws = scene.drawables.len() as u64;
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let mut metrics = RenderFrameMetrics::default();
        metrics.dropped_draws += dropped_3d_draws;
        for draw in &frame_draws {
            record_indexed_draw(
                &mut metrics,
                draw.geometry.index_count,
                draw.models.len(),
                2,
            );
        }
        // 世界 Sprite 通道（docs/54 §8-④）：与网格**共用相机、视口与深度附件**。
        // 这一段只做 CPU 侧准备（uniform + 缓冲容量），不碰 encoder，因此可以排在清屏之前，
        // 好让下面的通道计划拿到 `shared_depth` 这个事实。
        let world_view = if mesh.world_sprites.is_empty() {
            None
        } else {
            self.prepare_world_sprites(mesh, main_matrix, overlay_viewport, target)
        };
        // docs/54 §4.3：打开后处理时本帧所有场景通道都画进**离屏场景颜色纹理**，最后由
        // `PostProcess` 通道采样它写进目标附件。视图在这里"取出"、渲染完放回——这样渲染循环里
        // 不必同时持有 `&self`（画目标）与 `&mut self`（各通道的准备与状态）。
        let post_process = self.post_process;
        let scene_view = if post_process.is_some() {
            self.ensure_post_process_scene_view(target.0, target.1);
            self.post_process_scene_view.take()
        } else {
            None
        };
        let draw_target: &wgpu::TextureView = scene_view.as_ref().unwrap_or(view);
        // docs/54 §4.3：本帧跑哪些通道由**计划**决定，而不是散在下面的 `if` 里。
        // 计划是纯函数（`RenderPlan::for_frame`）⇒ 顺序与取舍可以在不启动 GPU 的单测里断言。
        let plan = RenderPlan::for_frame(FrameFacts {
            clear: clear.is_some(),
            has_3d_draws: !frame_draws.is_empty(),
            world_sprites: mesh.world_sprites.len(),
            world_sprite_opaque_segment: mesh
                .world_sprite_draws
                .iter()
                .any(|draw| draw.depth_write),
            world_sprite_shared_depth: world_view.is_some_and(|world| world.shared_depth),
            world_2d_draws: count_layer_draws(mesh, RenderLayer::World2D),
            overlay_2d_draws: count_layer_draws(mesh, RenderLayer::Overlay2D),
            screen_ui_draws: count_layer_draws(mesh, RenderLayer::ScreenUI),
            vector_text: !mesh.vector_vertices.is_empty() && self.storage_capable,
            post_process: post_process.is_some(),
        });
        // 矢量文字同理：能力不足时不声明该通道（计划里没有 ⇒ 不会绑占位管线），
        // 但顶点数要记成丢弃，否则就是"屏幕上少了字、日志里一个字都没有"。
        if !self.storage_capable && !mesh.vector_vertices.is_empty() {
            metrics.dropped_draws += 1;
        }
        let mut executed: Vec<RenderPass> = Vec::with_capacity(plan.len());
        if plan.contains(RenderPass::Clear)
            && let Some(clear) = clear
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: draw_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            executed.push(RenderPass::Clear);
        }
        // 深度附件是否已被本帧某个 pass 清过。共用深度时"谁先画谁清"、后面的 pass 只 Load；
        // 否则精灵与网格的深度就不是同一个空间里的值。
        let mut depth_cleared = false;
        // ① 不透明世界 Sprite：在网格 pass **之前**写深度 ⇒ 不透明精灵可以遮挡网格。
        if plan.contains(RenderPass::WorldSpriteOpaque)
            && let Some(world) = world_view.filter(|world| world.shared_depth)
        {
            self.draw_world_sprites(
                encoder,
                draw_target,
                target,
                world,
                DepthPass::WorldSpriteOpaque,
                &mesh.world_sprite_draws,
                &mesh.paths,
                &mesh.path_handles,
                SpritePassFilter::Opaque,
                true,
                &mut metrics,
            );
            depth_cleared = true;
            executed.push(RenderPass::WorldSpriteOpaque);
        }
        // P0-①：本帧的材质 bind group 在进入任何 pass 前一次性建好（阴影与网格共用同一份），
        // 环形缓冲先复位再填。此前是"每 Draw Call 每 Pass"分配 GPU buffer + bind group。
        let material_binds: Vec<wgpu::BindGroup> = if !frame_draws.is_empty()
            && (plan.contains(RenderPass::Shadow) || plan.contains(RenderPass::DepthOpaque))
        {
            self.material_arena_used = 0;
            self.instance_arena_used = 0;
            frame_draws
                .iter()
                .map(|draw| self.material_bind_group(draw.material, &draw.palette, &draw.models))
                .collect()
        } else {
            Vec::new()
        };
        if plan.contains(RenderPass::Shadow) {
            let mut shadow_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-shadow-pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            shadow_pass.set_pipeline(&self.shadow_pipeline);
            shadow_pass.set_bind_group(0, &self.shadow_bind_group, &[]);
            for (index, draw) in frame_draws.iter().enumerate() {
                shadow_pass.set_bind_group(1, &material_binds[index], &[]);
                shadow_pass.set_vertex_buffer(0, draw.geometry.vertex_buffer.slice(..));
                shadow_pass.set_index_buffer(
                    draw.geometry.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                shadow_pass.draw_indexed(
                    0..draw.geometry.index_count,
                    0,
                    0..draw.models.len() as u32,
                );
            }
            executed.push(RenderPass::Shadow);
        }
        if plan.contains(RenderPass::DepthOpaque) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-mesh3d-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: draw_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        // 不透明世界 Sprite 已经清过并写过这份深度；此时只 Load，网格与精灵
                        // 才在同一个深度空间里互相遮挡。
                        load: if depth_cleared {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(1.0)
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            depth_cleared = true;
            pass.set_pipeline(&self.mesh_pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            apply_declared_viewport(&mut pass, main_viewport, target);
            for (index, draw) in frame_draws.iter().enumerate() {
                pass.set_bind_group(1, &material_binds[index], &[]);
                pass.set_vertex_buffer(0, draw.geometry.vertex_buffer.slice(..));
                pass.set_index_buffer(
                    draw.geometry.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                pass.draw_indexed(0..draw.geometry.index_count, 0, 0..draw.models.len() as u32);
            }
            executed.push(RenderPass::DepthOpaque);
        }
        if plan.contains(RenderPass::WorldSprite)
            && let Some(world) = world_view
        {
            // ② 共用视图：半透明精灵在网格之后按画家顺序画，测深度但不写深度。
            // ③ 纯 2D / 宿主未声明视图（回滚档）：只有精灵之间的顺序，深度策略不参与，
            //    因此整段照旧按宿主排好的顺序画。
            let filter = if world.shared_depth {
                SpritePassFilter::Translucent
            } else {
                SpritePassFilter::All
            };
            self.draw_world_sprites(
                encoder,
                draw_target,
                target,
                world,
                DepthPass::WorldSprite,
                &mesh.world_sprite_draws,
                &mesh.paths,
                &mesh.path_handles,
                filter,
                !depth_cleared,
                &mut metrics,
            );
            executed.push(RenderPass::WorldSprite);
        }
        if plan.contains(RenderPass::Legacy2DOverlay) || plan.contains(RenderPass::ScreenUI) {
            let bytes = bytemuck::cast_slice(&mesh.vertices);
            // 2D 顶点缓冲按需增长。此前是固定 1 MiB + `if bytes.len() <= size`
            // **没有 else 分支**：超限时整个 2D 层（含 UI/文字）静默消失，日志里
            // 什么线索都没有。现在超限即扩容并留下可诊断的计数与日志。
            self.ensure_2d_vertex_capacity(bytes.len() as u64);
            self.queue.write_buffer(&self.vertex_buffer, 0, bytes);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-scene-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: draw_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            apply_declared_viewport(&mut pass, overlay_viewport, target);
            if let Some(font) = &mesh.font_atlas
                && self.ensure_font_atlas(font).is_none()
            {
                // 原来是 `let _ = ...`：上传失败的后果是 `font_atlas_bind` 一直为 None，
                // 于是**下面每一个 SDF 文字 draw 都被静默丢弃**——根因必须留在这里。
                tracing::warn!(
                    target: "render",
                    "[runtime] font atlas upload failed; SDF text draws will be dropped this frame"
                );
            }
            for font in &mesh.custom_font_atlases {
                if self.ensure_custom_font(&font.key, &font.pixels).is_none() {
                    tracing::warn!(
                        target: "render",
                        key = %font.key,
                        "[runtime] custom font upload failed; its text draws will be dropped this frame"
                    );
                }
            }
            // docs/54 §8-⑤：2D 叠加通道按 `RenderLayer` **分桶**后绘制
            // （World2D → Overlay2D → ScreenUI）。宿主本来就按这个顺序产出顶点，因此像素与
            // "照缓冲顺序画"逐字节一致；但层的先后从此由渲染器保证——HUD 不会再被一段插到它
            // 后面的调试线盖住，而这类错序类型检查抓不到。
            //
            // docs/54 §4.3：`Legacy2DOverlay`（World2D + Overlay2D 两个桶）与 `ScreenUI` 是计划里
            // 两条**独立通道**，因此这里按通道分桶；两者共用一次 `begin_render_pass`（同一条管线，
            // 拆成两个 pass 只多 CPU 开销、不写像素）。
            //
            // **保序合并（本片新增）**：相邻的、贴图与管线完全相同的 draw 合成**一次** `pass.draw`。
            // 能合的前提只有一条硬事实：顶点在同一个缓冲里**首尾相接**（
            // `start + count == 下一个 draw 的 start`），且键 =（贴图下标, 是否 SDF, 是否位图）相等。
            // 满足时合并后的光栅化与逐个画**逐字节相同**：三角形顺序、混合顺序都没变，
            // 所以这不是"为了快牺牲正确性"，而是把本来就该是一次提交的东西合成一次。
            // 为什么值得：2D 主路径原来**每个 `SceneDraw` 一次 draw call**，一千个相邻精灵就是
            // 一千次提交。`draw_calls` 与设备无关，软件光栅上也能量（本片读数就是这么取的）。
            for stage in [RenderPass::Legacy2DOverlay, RenderPass::ScreenUI] {
                if !plan.contains(stage) {
                    continue;
                }
                let mut batch: Option<OverlayBatch> = None;
                for layer in stage.overlay_layers() {
                    for index in layer_indices(&mesh.draws, |draw| draw.render_layer, *layer) {
                        let draw = &mesh.draws[index];
                        // 解析绑定组可能**改变贴图缓存**（异步加载在这一帧内完成上传）。键相同但
                        // 缓存变了，两次解析结果就可能是"白兜底"与"真贴图"两种——合并会把后者画成
                        // 前者。所以纪元一变就先冲刷批次：批次内的 draw 保证在同一个解析纪元里，
                        // 于是键相同 ⇒ 绑定的贴图内容相同。
                        let epoch = self.texture_epoch();
                        if batch.as_ref().is_some_and(|batch| batch.epoch != epoch) {
                            flush_overlay_batch(&mut pass, &mut metrics, &mut batch);
                        }
                        let (key, bind) = if draw.sdf {
                            // 文本绘制：共用 pipeline_sdf（shader 按顶点 sdf 值分支）。
                            // bitmap（sdf=2）：Nearest 采样 + MSDF 阈值化，小字号清晰。
                            // MSDF（sdf=1）：Linear 采样 + smoothstep AA，大字号平滑。
                            let bind = if draw.bitmap {
                                self.bind_group_for(draw.texture, &mesh.paths, &mesh.path_handles)
                            } else {
                                self.sdf_bind_for(draw.texture, &mesh.paths)
                            };
                            let Some(bind) = bind else {
                                // 画不出来必须留痕（见 `RenderFrameMetrics::dropped_draws`）：
                                // 静默 `continue` 的后果是"屏幕少一块、日志一个字没有"。
                                record_dropped_draw(&mut metrics);
                                tracing::warn!(
                                    target: "render",
                                    texture = draw.texture,
                                    bitmap = draw.bitmap,
                                    "[runtime] SDF text draw dropped: bind group unavailable"
                                );
                                continue;
                            };
                            (
                                OverlayBatchKey {
                                    texture: draw.texture,
                                    sdf: true,
                                    bitmap: draw.bitmap,
                                },
                                bind,
                            )
                        } else {
                            let Some(bind) =
                                self.bind_group_for(draw.texture, &mesh.paths, &mesh.path_handles)
                            else {
                                record_dropped_draw(&mut metrics);
                                tracing::warn!(
                                    target: "render",
                                    texture = draw.texture,
                                    path = ?draw
                                        .texture
                                        .checked_sub(1)
                                        .and_then(|index| mesh.paths.get(index as usize)),
                                    layer = ?draw.render_layer,
                                    "[runtime] draw dropped: bind group unavailable"
                                );
                                continue;
                            };
                            (
                                OverlayBatchKey {
                                    texture: draw.texture,
                                    sdf: false,
                                    bitmap: draw.bitmap,
                                },
                                bind,
                            )
                        };
                        let extends = batch.as_ref().is_some_and(|batch| {
                            batch.key == key && batch.start + batch.count == draw.start
                        });
                        if !extends {
                            flush_overlay_batch(&mut pass, &mut metrics, &mut batch);
                            pass.set_pipeline(if key.sdf {
                                &self.pipeline_sdf
                            } else {
                                &self.pipeline
                            });
                            pass.set_bind_group(0, &bind, &[]);
                            batch = Some(OverlayBatch {
                                key,
                                start: draw.start,
                                count: draw.count,
                                epoch,
                            });
                        } else if let Some(batch) = batch.as_mut() {
                            batch.count += draw.count;
                        }
                    }
                }
                flush_overlay_batch(&mut pass, &mut metrics, &mut batch);
                executed.push(stage);
            }
        }
        if plan.contains(RenderPass::VectorText)
            && !mesh.vector_vertices.is_empty()
            && !mesh.vector_glyphs.is_empty()
            && !mesh.vector_segments.is_empty()
        {
            let vertex_bytes = bytemuck::cast_slice(&mesh.vector_vertices);
            let glyph_bytes = bytemuck::cast_slice(&mesh.vector_glyphs);
            let segment_bytes = bytemuck::cast_slice(&mesh.vector_segments);
            self.ensure_vector_buffer_capacity(
                vertex_bytes.len() as u64,
                glyph_bytes.len() as u64,
                segment_bytes.len() as u64,
            );
            self.queue
                .write_buffer(&self.vector_vertex_buffer, 0, vertex_bytes);
            let glyph_hash = hash_bytes(glyph_bytes);
            if self.vector_glyph_hash != glyph_hash
                || self.vector_glyph_cache.len() != glyph_bytes.len()
            {
                self.queue
                    .write_buffer(&self.vector_glyph_buffer, 0, glyph_bytes);
                self.vector_glyph_cache.clear();
                self.vector_glyph_cache.extend_from_slice(glyph_bytes);
                self.vector_glyph_hash = glyph_hash;
            }
            let segment_hash = hash_bytes(segment_bytes);
            if self.vector_segment_hash != segment_hash
                || self.vector_segment_cache.len() != segment_bytes.len()
            {
                self.queue
                    .write_buffer(&self.vector_segment_buffer, 0, segment_bytes);
                self.vector_segment_cache.clear();
                self.vector_segment_cache.extend_from_slice(segment_bytes);
                self.vector_segment_hash = segment_hash;
            }
            if self.vector_bind_group.is_none() {
                self.vector_bind_group =
                    Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("rustlua-vector-text-bind-group"),
                        layout: &self.vector_bind_group_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: self.vector_glyph_buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: self.vector_segment_buffer.as_entire_binding(),
                            },
                        ],
                    }));
            }
            let bind = self.vector_bind_group.as_ref().expect("vector bind group");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-vector-text-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: draw_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.vector_pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, self.vector_vertex_buffer.slice(..));
            apply_declared_viewport(&mut pass, overlay_viewport, target);
            pass.draw(0..mesh.vector_vertices.len() as u32, 0..1);
            record_triangle_draw(&mut metrics, mesh.vector_vertices.len() as u32);
            executed.push(RenderPass::VectorText);
        }
        // docs/54 §4.3：最后一条通道——把离屏场景颜色按曝光/色调/暗角写进**目标附件**。
        if plan.contains(RenderPass::PostProcess)
            && let (Some(source), Some(settings)) = (scene_view.as_ref(), post_process)
        {
            let vertices = self.draw_post_process(encoder, source, view, settings, target);
            record_triangle_draw(&mut metrics, vertices as u32);
            executed.push(RenderPass::PostProcess);
        }
        // 把离屏视图放回（下一帧复用；纹理本体一直在 `self` 里，视图只是个句柄）。
        self.post_process_scene_view = scene_view;
        // 计划与执行核对（docs/54 §4.3）：计划驱动的开关如果被运行时条件绕开，就在这里留下
        // 明确的告警与可断言的差异。`last_frame_passes` 与 `last_frame_plan` 都是公开的，
        // 因此"这一帧到底跑了哪些通道"可以在测试里直接断言，不用去数 draw call 反推。
        if let Some(mismatch) = plan.mismatch(&executed) {
            tracing::warn!(
                target: "render",
                "[runtime] render plan mismatch: {mismatch}"
            );
        }
        self.last_frame_plan = plan;
        self.last_frame_passes = executed;
        metrics
    }
    /// 贴图**解析纪元**：这一组计数只要不变，同一个贴图键在这帧里就解析到同一份内容。
    ///
    /// 为什么需要它（而不是直接比较 `wgpu::BindGroup`）：wgpu 27 的 `BindGroup` 没有身份查询
    /// （`global_id` 是更晚版本才有的），而"同键同绑定"恰恰是 2D 保序合并的安全前提。
    /// 换个角度想：绑定组只可能因为**缓存被写入**而改变——异步解码完成后的上传（`textures` 变长）、
    /// 自定义字体图集上传（`custom_fonts` 变长）、内建字体图集首次绑定（布尔翻转）。
    /// 这三件事覆盖了 `bind_group_for`/`sdf_bind_for` 的全部可变来源，所以把它们当纪元就够：
    /// 纪元不变 ⇒ 键相同 ⇒ 内容相同。
    ///
    /// 注意这里**故意**不含 `texture_identity_hits/misses`：那两个是诊断计数，命中与否不改变结果。
    fn texture_epoch(&self) -> (usize, usize, bool) {
        (
            self.textures.len(),
            self.custom_fonts.len(),
            self.font_atlas_bind.is_some(),
        )
    }
}

/// 2D 保序合并的键：**决定管线与绑定组的那几项**。
///
/// 只有这三项相同、且顶点首尾相接，两次 draw 才能合成一次提交。`bitmap` 与 `sdf` 都必须在键里：
/// 前者换绑定组（Nearest vs Linear 采样），后者换管线（SDF shader vs 普通 2D shader）。
#[derive(Clone, Copy, PartialEq, Eq)]
struct OverlayBatchKey {
    texture: u32,
    sdf: bool,
    bitmap: bool,
}

/// 当前正在累积的 2D 批次：键 + 顶点区间 + **解析纪元**。
///
/// 纪元是安全的另一半：同一个键在不同纪元下可能解析到不同内容（异步贴图恰好在这一帧完成上传，
/// 前一次拿到白兜底、后一次拿到真贴图）。批次内只允许同一纪元，于是"键相同"就真的意味着
/// "绑定的贴图内容相同"。
struct OverlayBatch {
    key: OverlayBatchKey,
    start: u32,
    count: u32,
    epoch: (usize, usize, bool),
}

/// 冲刷当前 2D 批次：一次 `pass.draw` 覆盖整段**连续**顶点，并记一次 draw call。
///
/// 注意管线/绑定组是在**开批**时设的（见调用点），这里只提交绘制——所以冲刷本身不改状态，
/// 这也是"合并不改像素"的实现保证：状态切换只发生在键变化处，而那些位置本来就要切。
fn flush_overlay_batch(
    pass: &mut wgpu::RenderPass<'_>,
    metrics: &mut RenderFrameMetrics,
    batch: &mut Option<OverlayBatch>,
) {
    if let Some(batch) = batch.take() {
        pass.draw(batch.start..batch.start + batch.count, 0..1);
        record_vertex_draw(metrics, batch.count);
    }
}
