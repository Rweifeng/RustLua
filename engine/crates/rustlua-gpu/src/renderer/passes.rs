//! 渲染通道的执行侧（WP12 阶段 4c 从 `renderer/mod.rs` 原样搬出）：`render`、
//! `render_into_texture`、后处理通道 `draw_post_process`，以及 446 行的 `render_passes`。
//!
//! 计划 §5 的硬规矩 2：**只搬位置，不在本工作包内拆函数体** —— `render_passes` 按原样整体搬，
//! 拆成 `passes/clear.rs` / `passes/world2d.rs` 之类属于改变结构，另开工作包（§9）。
//! `Renderer` 结构体与字段仍留在 `renderer/mod.rs`；被父模块调用的私有方法按编译器要求放宽。

use rustlua_camera::camera::{ClearPolicy, DEFAULT_3D_CLEAR, RenderView};
use rustlua_infra::logging::{EngineError, LogModule};
use rustlua_math::geom3d::Frustum;
use rustlua_math::math3d::{Aabb3, Vec3};
use rustlua_render::frame::{Scene3d, Scene3dDrawable, SceneMesh};
use rustlua_render::layer::{RenderLayer, layer_indices};
use rustlua_render::mesh3d::MaterialAlphaMode;
use rustlua_render::post_process::PostProcess;
use rustlua_render::render_plan::{FrameFacts, RenderPass, RenderPlan};
use rustlua_render::render_settings::{IblMode, SceneColorFormat, hdr_activation};
use rustlua_render::terrain::TerrainSpec;
use rustlua_render::water::WaterSpec;
// M73-00 WP5：时钟必须走 `web-time` 而不是 `std::time`。`wasm32-unknown-unknown` 的 std
// 落在 `sys/pal/unsupported` 上，`Instant::now()` 会**直接 panic**（"time not implemented on
// this platform"，见 std 源码 `sys/time/unsupported.rs`）——渲染器是第一个真在 wasm 上被调用的
// 引擎子系统，帧计时（`cpu_frame_us`）就会踩到它。`web-time` 在原生上**就是** `std::time::Instant`
// 的再导出（类型相同、行为为零变化），在 wasm 上换成 `performance.now()`。
use web_time::Instant;
use wgpu::SurfaceError;

use super::world_sprite::SpritePassFilter;

use super::pass_timestamps::{PassTimestampLayout, PassTimestamps};
use super::pipeline_cache::{ColorTarget, ShaderVariantKey, Shading, SkinMode};
use super::{
    DepthPass, RenderFrameMetrics, RenderOutcome, Renderer, apply_declared_viewport,
    count_layer_draws, hash_bytes, point_shadow_far, point_shadow_light_indices,
    point_shadow_view_projections, record_dropped_draw, record_indexed_draw, record_set_pass,
    record_triangle_draw, record_vertex_draw, shadow_view_projections,
};

/// 程序化天空的太阳项强度。Tonemap 的 `params[57]` 与 SSR 天空回退的 `params.sky.w` 共用
/// 同一个数：两处各写一份会让反射里的太阳和天上的太阳亮度对不上，而这个值不来自
/// `RenderSettings`，只有一个真值。
const SKY_SUN_INTENSITY: f32 = 2.4;

/// P3-R189/R191：组包围盒是否与视锥相交（包围球保守判定）。阴影级联与平面反射共用。
fn frustum_may_contain(frustum: Frustum, bounds: &Aabb3) -> bool {
    let center = (bounds.min + bounds.max) * 0.5;
    let half = (bounds.max - bounds.min) * 0.5;
    let radius = (half.x * half.x + half.y * half.y + half.z * half.z).sqrt();
    frustum.contains_sphere(center, radius)
}
fn point_to_aabb_distance_sq(bounds: &Aabb3, point: Vec3) -> f32 {
    let dx = (bounds.min.x - point.x)
        .max(point.x - bounds.max.x)
        .max(0.0);
    let dy = (bounds.min.y - point.y)
        .max(point.y - bounds.max.y)
        .max(0.0);
    let dz = (bounds.min.z - point.z)
        .max(point.z - bounds.max.z)
        .max(0.0);
    dx * dx + dy * dy + dz * dz
}

pub(crate) fn water_plane_from_drawables(drawables: &[Scene3dDrawable]) -> Option<[f32; 4]> {
    for drawable in drawables {
        if !WaterSpec::is_water_path(&drawable.mesh_path) {
            continue;
        }
        let spec = WaterSpec::parse(&drawable.mesh_path).ok()?;
        let columns = &drawable.model.columns;
        let scale = |column: &[f32; 4]| {
            (column[0] * column[0] + column[1] * column[1] + column[2] * column[2]).sqrt()
        };
        let sx = scale(&columns[0]);
        let sy = scale(&columns[1]);
        let sz = scale(&columns[2]);
        return Some([
            columns[3][1] + spec.level * sy,
            spec.size * 0.5 * sx.max(sz),
            columns[3][0],
            columns[3][2],
        ]);
    }
    None
}

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

    /// P3-R01：把 HDR 场景颜色（线性、可 >1）经曝光与曲线写进 LDR 目标。
    ///
    /// Tonemap 专用布局：HDR 颜色 + 采样器 + 参数 + 深度 + 逆视图投影（SSAO）。
    /// 输出附件用 Clear 起步：全屏三角形覆盖整个目标，结果与"上一帧残留"无关。
    // 效果开关逐个传参：打包成结构体反而让调用点看不出这一帧到底开了哪些 pass。
    #[allow(clippy::too_many_arguments)]
    fn draw_tonemap(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
        settings: PostProcess,
        fxaa: bool,
        ssao: bool,
        ssr: bool,
        planar_reflection: bool,
        contact_shadows: bool,
        fog: bool,
        fog_density: f32,
        fog_color: [f32; 3],
        god_rays: bool,
        dof: bool,
        dof_focus: f32,
        dof_range: f32,
        dof_strength: f32,
        motion_blur: f32,
        scene: Option<&Scene3d>,
        target_size: (u32, u32),
    ) -> u64 {
        // 384B：8 个标量 + 视图投影 + 逆视图投影 + 相机位置 + 指向光的方向 + 雾 + 景深 + 天空
        // + 水面反射 + 云层 + 剖面裁剪平面 + 世界→局部 + 地形范围（P3-R166）。
        // SSR/contact shadows/fog/天空用后几者把深度重建到世界空间；没有相机时保持全零，
        // shader 直接跳过。
        let mut params = [0.0_f32; super::TONEMAP_UNIFORM_FLOATS];
        params[0] = settings.exposure_ev + self.exposure_offset;
        params[1] = settings.tonemap.code();
        params[2] = settings.bloom;
        params[3] = if fxaa { 1.0 } else { 0.0 };
        params[4] = if ssao { 1.0 } else { 0.0 };
        params[5] = if ssr { 1.0 } else { 0.0 };
        params[6] = if contact_shadows { 1.0 } else { 0.0 };
        params[7] = if god_rays { 1.0 } else { 0.0 };
        // P3-R80：sky.w 携带云量（0..1），体积云密度阈值按它偏移。
        params[59] = self.cloud_coverage.clamp(0.0, 1.0);
        // P3-R82：water_params.w（保留槽）携带云速倍率。
        params[67] = self.cloud_speed.clamp(0.0, 4.0);
        params[68] = self.cloud_base.clamp(40.0, 200.0);
        params[69] = self.flash_intensity.clamp(0.0, 1.0);
        params[70] = self.vignette.clamp(0.0, 1.0);
        params[71] = self.grain.clamp(0.0, 1.0);
        // P3-R174：本帧视图投影要留到下一帧当「上一帧」，运动模糊按它重投影。
        let mut current_view_projection = None;
        if let Some(scene) = scene
            && let Ok(view_projection) = scene.camera.view_projection_matrix(scene.view.viewport)
        {
            current_view_projection = Some(view_projection);
            params[8..24].copy_from_slice(&view_projection.columns.concat());
            if let Ok(inverse) = view_projection.inverse() {
                params[24..40].copy_from_slice(&inverse.columns.concat());
            }
            params[40] = scene.camera.position.x;
            params[41] = scene.camera.position.y;
            params[42] = scene.camera.position.z;
            // PBR 用 `l = -light.direction`，这里存“指向光”的单位向量。
            params[44] = -scene.light.direction.x;
            params[45] = -scene.light.direction.y;
            params[46] = -scene.light.direction.z;
            params[47] = params[6];
        }
        if fog && fog_density.is_finite() && fog_density > 0.0 {
            params[48] = fog_density;
            params[49] = fog_color[0].clamp(0.0, 1.0);
            params[50] = fog_color[1].clamp(0.0, 1.0);
            params[51] = fog_color[2].clamp(0.0, 1.0);
        }
        if dof {
            params[52] = 1.0;
            params[53] = if dof_focus.is_finite() {
                dof_focus.max(0.0)
            } else {
                20.0
            };
            params[54] = if dof_range.is_finite() {
                dof_range.max(0.1)
            } else {
                30.0
            };
            params[55] = if dof_strength.is_finite() {
                dof_strength.clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
        // 天空只在“有场景相机”的 HDR 帧启用；与雾共用开关。
        if fog && scene.is_some() {
            params[56] = 1.0;
            params[57] = SKY_SUN_INTENSITY;
            params[58] = 0.55;
        }
        // P3-R52：取第一块 `__water__` 的世界平面（level/半宽/中心）+ 时钟，
        // 让 Tonemap 能对水面像素做屏幕空间物体反射。没有水面时 enable=0。
        if let Some(plane) = scene.and_then(|scene| water_plane_from_drawables(&scene.drawables)) {
            params[60] = plane[0];
            params[61] = plane[1];
            params[62] = plane[2];
            params[63] = plane[3];
            params[64] = 1.0;
            params[65] = self.clock_secs();
            params[66] = 0.55;
            // P3-R171：逐通道消光系数（线性 RGB，每米）+ 强度。红光衰减最快、
            // 蓝光最慢，所以浅滩透亮、深水收向青蓝；没有水面时 w=0，着色器整段跳过。
            params[100] = 0.55;
            params[101] = 0.20;
            params[102] = 0.14;
            params[103] = 1.0;
            // P3-R172：水体散射色（线性 RGB）+ 强度。吸收把河床乘暗，散射把深水拉回
            // 水体自身的颜色；权重是 1 − T，浅滩几乎不受影响。
            params[104] = 0.004;
            params[105] = 0.030;
            params[106] = 0.040;
            params[107] = 1.0;
            // P3-R173：相机在水面矩形内且低于水位 ⇒ 整帧隔着水，光程取相机到该像素
            // 世界点的距离（与 P3-R171 的河床深度不同）。上限 40 m 防止天空像素被
            // 反投影到远点后把整帧压成纯水体色。
            let underwater = params[41] < plane[0]
                && (params[40] - plane[2]).abs() <= plane[1]
                && (params[42] - plane[3]).abs() <= plane[1];
            params[108] = if underwater { 1.0 } else { 0.0 };
            params[109] = 40.0;
            // P3-R207：深度加权折射档。关闭时 z=0 ⇒ 着色器仍用旧常数 0.0035（默认像素逐字节不变）；
            // 开档后基准沿用同一个常数，只叠加「光程 × 掠射角」那一项，保证两档差异只来自深度。
            if self.water_refraction_gain > 0.0 {
                params[110] = 0.0035;
                params[111] = self.water_refraction_gain;
            }
            // P3-R208：水下视角的界面档。只有相机入水（上面那个开关）且档位 > 0 才写，
            // 默认四个槽恒零 ⇒ 着色器走回 P3-R173 的原式，像素逐字节不变。
            // y = 水 → 空气的折射率比（1/1.333 ⇒ 临界角约 48.6°），
            // z = 界面替换强度，w = 临界角的过渡带宽（按余弦，0.06 ≈ ±10°）。
            if underwater && self.submerged_view_gain > 0.0 {
                params[132] = 1.0;
                params[133] = 1.0 / 1.333;
                params[134] = self.submerged_view_gain;
                params[135] = 0.06;
            }
            // P3-R241：水面折射的屏幕空间 Snell 步进。只在档位 > 0 时写；默认四槽恒零 ⇒
            // 着色器走 R207 的偏移近似，像素逐字节不变。y = 空气 → 水的折射率比，
            // z = 首步步长（米，水柱里步长按 1 + 0.35·i 递增），w = 命中过冲的厚度容差（米）。
            if self.water_refract_steps > 0 {
                params[136] = self.water_refract_steps as f32;
                params[137] = 1.0 / 1.333;
                params[138] = 0.35;
                params[139] = 1.0;
            }
        }
        // P3-R170：平面反射图可用时，着色器优先按投影 uv 采样它（真实镜像场景），
        // 未命中再回退到 `water_reflection` 的屏幕空间步进。
        if planar_reflection {
            params[96] = 1.0;
            params[97] = 0.55;
        }
        // P3-R166：`sect=1` 地形 + 相机裁剪平面 ⇒ Tonemap 把切口封成实体土壤剖面。
        // 只在两者同时存在时启用；高度图随网格缓存（按资源身份取），这里只克隆视图句柄。
        let mut section_view: Option<wgpu::TextureView> = None;
        if let Some(scene) = scene {
            let clip = scene.camera.clip_plane;
            if clip[0] * clip[0] + clip[1] * clip[1] + clip[2] * clip[2] > 1e-8 {
                for drawable in &scene.drawables {
                    if !TerrainSpec::is_terrain_path(&drawable.mesh_path) {
                        continue;
                    }
                    let Some(cached) = self.cached_mesh(drawable.mesh_handle, &drawable.mesh_path)
                    else {
                        continue;
                    };
                    let Some(map) = cached.section_height.as_ref() else {
                        continue;
                    };
                    let Ok(mut world_to_local) = drawable.model.inverse() else {
                        continue;
                    };
                    // 分块地形的高度图烘的是整张地形：把块心补回平移列后，
                    // 世界点才落在 `h=height_map(u=x/size+0.5)` 的同一套局部坐标里。
                    world_to_local.columns[3][0] += map.origin[0];
                    world_to_local.columns[3][2] += map.origin[1];
                    params[72..76].copy_from_slice(&clip);
                    params[76..92].copy_from_slice(&world_to_local.columns.concat());
                    params[92] = map.min_y;
                    params[93] = map.span;
                    params[94] = map.half;
                    params[95] = 1.0;
                    section_view = Some(map.view.clone());
                    break;
                }
            }
        }
        // P3-R174：相机运动模糊。上一帧的视图投影把当前像素的世界点投回屏幕，与当前 uv
        // 的差就是这一像素一帧内扫过的屏幕距离。没有上一帧（首帧/刚打开）或强度 0 时
        // 整段不写，着色器跳过 —— 相机静止时重投影回同一点，帧逐字节不变。
        // x = 强度，y = 屏幕速度上限（uv，防止大位移抽到画面另一侧），w = 开关。
        if motion_blur > 0.0
            && let Some(previous) = self.tonemap_prev_view_projection
        {
            params[112..128].copy_from_slice(&previous.columns.concat());
            params[128] = motion_blur.clamp(0.0, 1.0);
            params[129] = 0.02;
            params[131] = 1.0;
        }
        self.tonemap_prev_view_projection = current_view_projection;
        let section_height_view = section_view.as_ref().unwrap_or(&self.section_height_white);
        self.queue
            .write_buffer(&self.tonemap_params, 0, bytemuck::cast_slice(&params));
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-tonemap-bind-group"),
            layout: &self.tonemap_layout,
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
                    resource: self.tonemap_params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.ssao_depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.ssao_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&self.post_process_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&self.ssr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(&self.post_process_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(section_height_view),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&self.planar_reflection_view),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Sampler(&self.post_process_sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rustlua-tonemap-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(
            self.tonemap_pipeline
                .as_ref()
                .expect("tonemap pipeline checked at activation"),
        );
        pass.set_bind_group(0, &bind_group, &[]);
        apply_declared_viewport(&mut pass, None, target_size);
        pass.draw(0..3, 0..1);
        3
    }

    pub fn render(&mut self, mesh: &SceneMesh, scene3d: Option<&Scene3d>) -> RenderOutcome {
        let Some(surface) = &self.surface else {
            tracing::warn!(target: "game", "[render] render() requires a window surface; use render_into_texture");
            return RenderOutcome::Skipped;
        };
        // P3-R180：acquire 单列计时。它是宿主 `submit` 阶段的第一段。
        let acquire_started = Instant::now();
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
        let acquire_us = acquire_started.elapsed().as_micros() as u64;
        let started = Instant::now();
        let completed_gpu_us = self.poll_gpu_timestamp();
        let timestamp_slot = self.acquire_timestamp_slot();
        if timestamp_slot.is_some()
            && let Some(query_set) = &self.timestamp_query_set
        {
            encoder.write_timestamp(query_set, 0);
        }
        let record_started = Instant::now();
        let (mut metrics, timestamp_layout) = self.render_passes(
            &mut encoder,
            mesh,
            scene3d,
            &view,
            (self.config.width, self.config.height),
            true,
        );
        metrics.cpu_record_us = record_started.elapsed().as_micros() as u64;
        if let Some(slot) = timestamp_slot {
            self.finish_gpu_timestamp(&mut encoder, slot, &timestamp_layout);
        }
        let submit_started = Instant::now();
        let submission = self.queue.submit([encoder.finish()]);
        metrics.cpu_submit_us = submit_started.elapsed().as_micros() as u64;
        if let Some(slot) = timestamp_slot {
            self.arm_timestamp_slot(slot, submission, timestamp_layout);
        }
        // `cpu_frame_us` 保持历史口径（acquire + 录制 + submit，不含 present）：改它会让
        // docs/53 与既有基准的读数失去可比性。present 单列在 `cpu_present_us`。
        metrics.cpu_frame_us = started.elapsed().as_micros() as u64;
        metrics.gpu_frame_us = completed_gpu_us;
        metrics.cpu_acquire_us = acquire_us;
        self.last_frame_metrics = metrics;
        let present_started = Instant::now();
        output.present();
        self.last_frame_metrics.cpu_present_us = present_started.elapsed().as_micros() as u64;
        RenderOutcome::Presented
    }

    /// Renders the current frame into an offscreen texture view (no present).
    /// The caller owns the target texture and supplies its view + dimensions;
    /// the depth attachment is rebuilt to match the target size.
    /// 半分辨率 SSAO 目标随渲染尺寸重建；1×1 白图是关闭/未执行时的安全默认。
    fn ensure_ssao_target(&mut self, width: u32, height: u32) {
        let size = (width.max(1).div_ceil(2), height.max(1).div_ceil(2));
        if self.ssao_size == size {
            return;
        }
        let create = |label: &str| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        self.ssao_raw_view = create("rustlua-ssao-raw");
        self.ssao_view = create("rustlua-ssao-blur");
        self.ssao_size = size;
    }

    fn ensure_ssr_target(&mut self, width: u32, height: u32) {
        let size = (width.max(1).div_ceil(2), height.max(1).div_ceil(2));
        if self.ssr_size == size {
            return;
        }
        let create = |label: &str, _copy_src: bool| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let raw = create("rustlua-ssr-raw", false);
        let current = create("rustlua-ssr-current", true);
        let history = create("rustlua-ssr-history", true);
        self.ssr_raw_view = raw.create_view(&wgpu::TextureViewDescriptor::default());
        self.ssr_view = current.create_view(&wgpu::TextureViewDescriptor::default());
        self.ssr_history_view = history.create_view(&wgpu::TextureViewDescriptor::default());
        self.ssr_texture = current;
        self.ssr_history_texture = history;
        self.ssr_prev_view_projection = None;
        self.ssr_size = size;
    }

    /// 半分辨率 SSR：读 HDR 场景颜色与深度，写 `ssr_view`。
    fn draw_ssr(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        scene: &Scene3d,
        target: (u32, u32),
    ) -> bool {
        let Ok(view_projection) = scene.camera.view_projection_matrix(scene.view.viewport) else {
            return false;
        };
        let Ok(inverse) = view_projection.inverse() else {
            return false;
        };
        self.ensure_ssr_target(target.0, target.1);
        let mut params = [0.0_f32; 44];
        params[0..16].copy_from_slice(&inverse.columns.concat());
        params[16..32].copy_from_slice(&view_projection.columns.concat());
        params[32] = scene.camera.position.x;
        params[33] = scene.camera.position.y;
        params[34] = scene.camera.position.z;
        // P3-R176：天空回退的方向源。指向光的单位向量与 Tonemap 的 `params[44..47]` 同一口径；
        // 太阳强度同时是开关——天空关掉时写 0，未命中的反射保持「零 = 黑」的旧行为。
        if self.render_settings.fog {
            let direction = scene.light.direction;
            let length =
                (direction.x * direction.x + direction.y * direction.y + direction.z * direction.z)
                    .sqrt()
                    .max(1e-6);
            params[36] = -direction.x / length;
            params[37] = -direction.y / length;
            params[38] = -direction.z / length;
            params[39] = SKY_SUN_INTENSITY;
            // P3-R178：云量/云速/时间/云底，与 Tonemap 体积云读的四个值同源
            // （`params[59]/[67]/[65]/[68]`），反射的天空才有同一片云。
            params[40] = self.cloud_coverage.clamp(0.0, 1.0);
            params[41] = self.cloud_speed.clamp(0.0, 4.0);
            params[42] = self.clock_secs();
            params[43] = self.cloud_base.clamp(40.0, 200.0);
        }
        self.queue
            .write_buffer(&self.ssr_params, 0, bytemuck::cast_slice(&params));
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-ssr-bind-group"),
            layout: &self.ssr_bind_group_layout,
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
                    resource: wgpu::BindingResource::TextureView(&self.ssao_depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.ssr_params.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rustlua-ssr-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.ssr_raw_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.ssr_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        let mut blur_params = [0.0_f32; 36];
        blur_params[0] = 1.0 / self.ssr_size.0 as f32;
        blur_params[1] = 1.0 / self.ssr_size.1 as f32;
        blur_params[3] = if self.ssr_prev_view_projection.is_some() {
            1.0
        } else {
            0.0
        };
        blur_params[4..20].copy_from_slice(&inverse.columns.concat());
        let previous = self.ssr_prev_view_projection.unwrap_or(view_projection);
        blur_params[20..36].copy_from_slice(&previous.columns.concat());
        self.queue
            .write_buffer(&self.ssr_blur_params, 0, bytemuck::cast_slice(&blur_params));
        let blur_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-ssr-blur-bind-group"),
            layout: &self.ssr_blur_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.ssr_raw_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.ssao_depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.ssr_blur_params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.ssr_history_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.post_process_sampler),
                },
            ],
        });
        let mut blur_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rustlua-ssr-blur-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.ssr_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        blur_pass.set_pipeline(&self.ssr_blur_pipeline);
        blur_pass.set_bind_group(0, &blur_bind_group, &[]);
        blur_pass.draw(0..3, 0..1);
        self.ssr_prev_view_projection = Some(view_projection);
        true
    }

    /// 在 Tonemap 前把半分辨率 AO 写进 `ssao_view`；没有相机/尺寸时跳过。
    fn draw_ssao(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene3d,
        target: (u32, u32),
    ) -> bool {
        let Ok(view_projection) = scene.camera.view_projection_matrix(scene.view.viewport) else {
            return false;
        };
        let Ok(inverse) = view_projection.inverse() else {
            return false;
        };
        self.ensure_ssao_target(target.0, target.1);
        let mut params = [0.0_f32; 36];
        params[0..16].copy_from_slice(&inverse.columns.concat());
        params[16..32].copy_from_slice(&view_projection.columns.concat());
        params[32] = scene.camera.position.x;
        params[33] = scene.camera.position.y;
        params[34] = scene.camera.position.z;
        self.queue
            .write_buffer(&self.ssao_params, 0, bytemuck::cast_slice(&params));
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-ssao-bind-group"),
            layout: &self.ssao_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.ssao_depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.ssao_params.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rustlua-ssao-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.ssao_raw_view,
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
        pass.set_pipeline(&self.ssao_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        let blur_params = [
            1.0 / self.ssao_size.0 as f32,
            1.0 / self.ssao_size.1 as f32,
            0.0,
            0.0,
        ];
        self.queue.write_buffer(
            &self.ssao_blur_params,
            0,
            bytemuck::cast_slice(&blur_params),
        );
        let blur_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-ssao-blur-bind-group"),
            layout: &self.ssao_blur_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.ssao_raw_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.ssao_depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.ssao_blur_params.as_entire_binding(),
                },
            ],
        });
        let mut blur_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rustlua-ssao-blur-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.ssao_view,
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
        blur_pass.set_pipeline(&self.ssao_blur_pipeline);
        blur_pass.set_bind_group(0, &blur_bind_group, &[]);
        blur_pass.draw(0..3, 0..1);
        true
    }

    pub fn render_into_texture(
        &mut self,
        mesh: &SceneMesh,
        scene3d: Option<&Scene3d>,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        include_ui: bool,
    ) -> RenderOutcome {
        self.ensure_depth_view(width, height);
        if self.render_settings.ssao {
            self.ensure_ssao_target(width, height);
        }
        if self.render_settings.ssr {
            self.ensure_ssr_target(width, height);
        }
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
        let record_started = Instant::now();
        let (mut metrics, timestamp_layout) = self.render_passes(
            &mut encoder,
            mesh,
            scene3d,
            view,
            (width, height),
            include_ui,
        );
        metrics.cpu_record_us = record_started.elapsed().as_micros() as u64;
        if self.render_settings.ssr {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.ssr_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &self.ssr_history_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: self.ssr_size.0,
                    height: self.ssr_size.1,
                    depth_or_array_layers: 1,
                },
            );
        }
        if let Some(slot) = timestamp_slot {
            self.finish_gpu_timestamp(&mut encoder, slot, &timestamp_layout);
        }
        let submit_started = Instant::now();
        let submission = self.queue.submit([encoder.finish()]);
        metrics.cpu_submit_us = submit_started.elapsed().as_micros() as u64;
        if let Some(slot) = timestamp_slot {
            self.arm_timestamp_slot(slot, submission, timestamp_layout);
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
        include_ui: bool,
    ) -> (RenderFrameMetrics, PassTimestampLayout) {
        // E06-3：查询集暂时移出字段，避免 `&mut self` 的借用与跨通道写入冲突；函数末尾归还。
        let timestamp_query_set = self.timestamp_query_set.take();
        let mut pass_timestamps = PassTimestamps::new(timestamp_query_set.as_ref());
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
        // docs/54 §8-③：本帧世界视图的投影矩阵**只有一份来源**——宿主声明的 `RenderView`。
        // 渲染器不再自己回查 `Camera3D`（那只是"宿主没声明"时的历史回退），回退吃的是
        // 场景提取时就已按视口烘焙好的 `Scene3d.view`（拆分批 7）。
        let main_matrix = self
            .frame_views
            .main
            .and_then(RenderView::matrix)
            .or_else(|| scene3d.and_then(|scene| scene.view.matrix()));
        // P3-R255：烘焙用的主视图视锥只在这里打包一次（与网格吃同一份 `main_matrix`）；
        // 关档时恒 `None`，arena 侧连视锥参数都不传。
        self.bake_frustum = if self.gpu_motion_frustum_cull {
            main_matrix.map(|matrix| {
                let mut packed = [[0.0_f32; 4]; 6];
                for (slot, plane) in Frustum::from_view_projection(matrix)
                    .planes()
                    .iter()
                    .enumerate()
                {
                    packed[slot] = [
                        plane.normal.x,
                        plane.normal.y,
                        plane.normal.z,
                        plane.distance,
                    ];
                }
                packed
            })
        } else {
            None
        };
        // 3D 网格/阴影两条路不再看设备能力：没有存储缓冲时（WebGL2）实例模型矩阵改走
        // `@binding(2)` 的 64 字节 uniform，`frame_3d_draws` 因实例化关闭本来就一条 draw
        // 一个模型（见 `construct.rs` 的 `InstanceMode`）。
        let frame_draws = if let Some(scene) = scene3d {
            self.update_3d_uniforms(scene, main_matrix);
            self.frame_3d_draws(scene)
        } else {
            // P3-R245：纯 2D 帧没有 3D 提取可认领暂存批次——直接丢弃，不跨帧残留。
            self.staged_motion.clear();
            Vec::new()
        };
        // P3-R02：着色轴由本帧设置决定。`set_render_settings` 已经负责在首次开启 PBR 时
        // 编译变体；这里再兜一次底，保证绕过 setter 直接改字段的实现路径也不会去查空缓存
        // （`ensure_pbr_pipelines` 是幂等的，重复调用只读一个 bool）。
        if self.render_settings.pbr {
            self.ensure_pbr_pipelines();
        }
        let shading = if self.render_settings.pbr {
            Shading::Pbr
        } else {
            Shading::Legacy
        };
        // P3-R03-2：IBL 档位是策略 × 环境 × 设备能力的纯解析结果，本帧所有网格共享同一档。
        let ibl = if shading == Shading::Pbr {
            self.ibl_mode()
        } else {
            IblMode::Off
        };
        // P3-R183：剔除计数来自刚结束的 `frame_3d_draws`（纯 2D 帧两者皆 0）。
        let mut metrics = RenderFrameMetrics {
            cull_candidates: self.frame_cull_candidates,
            cull_frustum: self.frame_cull_frustum,
            ..RenderFrameMetrics::default()
        };
        // P3-R256：阴影投射者的距离预算（只掐阴影链，0 = 关档）。参考点与逐实例距离
        // 剔除一致：本帧相机世界位置（`update_3d_uniforms` 写入）。
        let shadow_caster_budget = self.shadow_caster_max_distance;
        let shadow_caster_budget_sq = shadow_caster_budget * shadow_caster_budget;
        let shadow_caster_camera = self.camera_world_position;
        for draw in &frame_draws {
            record_indexed_draw(
                &mut metrics,
                draw.geometry.index_count,
                draw.geometry.vertex_count,
                draw.instance_count(),
                2,
            );
        }
        // 世界 Sprite 通道（docs/54 §8-④）：与网格**共用相机、视口与深度附件**。
        // 这一段只做 CPU 侧准备（uniform + 缓冲容量），不碰 encoder，因此可以排在清屏之前，
        // 好让下面的通道计划拿到 `shared_depth` 这个事实。
        // M74-03 P2：GPU 粒子复用同一份世界视图/相机 uniform，因此它单独存在时也要准备这份视图。
        let particles_active = self.compute_enabled()
            && self
                .gpu_particles
                .iter()
                .any(super::gpu_particles::GpuParticleSystem::is_active);
        let world_view = if mesh.world_sprites.is_empty() && !particles_active {
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
        // P3-R01：HDR 是否真的启用由**策略 + 能力 + 本帧内容 + 曝光归属**四件事共同决定。
        // 纯 2D 帧不创建 HDR 目标；后处理自带曝光（`exposure != 1.0`）时回退 LDR，避免同一份
        // 曝光被 Tonemap 与后处理乘两次。降级必须留痕：策略允许却没能启用时告警一次。
        let scene_content =
            !frame_draws.is_empty() || !mesh.world_sprites.is_empty() || particles_active;
        let hdr_capable = self.capabilities.hdr_usable()
            && self
                .pipelines
                .get(&ShaderVariantKey::mesh(
                    shading,
                    SkinMode::Off,
                    ColorTarget::Hdr,
                ))
                .is_some()
            && self.hdr_world_sprite_pipeline.is_some()
            && self.hdr_world_sprite_opaque_pipeline.is_some()
            && self.tonemap_pipeline.is_some();
        let mut hdr_active = hdr_activation(
            self.render_settings,
            hdr_capable,
            scene_content,
            post_process.map(|settings| settings.exposure),
        );
        if self.render_settings.hdr_allowed()
            && scene_content
            && !hdr_active
            && !self.hdr_warning_logged
        {
            self.hdr_warning_logged = true;
            let reason = if !self.capabilities.hdr_usable() {
                self.capabilities
                    .hdr_unavailable_reason()
                    .unwrap_or_else(|| "HDR 能力不可用".to_owned())
            } else if post_process.is_some_and(|settings| settings.exposure != 1.0) {
                "HDR 模式要求 PostProcess.exposure == 1.0（曝光只走 exposure_ev）；本帧回退 LDR"
                    .to_owned()
            } else {
                "HDR 管线未构造".to_owned()
            };
            tracing::warn!(
                target: "render",
                reason = %reason,
                "[runtime] HDR 未启用，本帧回退 LDR"
            );
        }
        let hdr_view = if hdr_active {
            self.ensure_hdr_scene_view(target.0, target.1);
            self.hdr_scene_view.take()
        } else {
            None
        };
        if hdr_active && hdr_view.is_none() {
            hdr_active = false;
            tracing::warn!(
                target: "render",
                "[runtime] HDR 场景视图缺失，本帧回退 LDR"
            );
        }
        // 3D / 世界 Sprite / 粒子写 `scene_target`；Tonemap 之后再写 `draw_target`（LDR）。
        let scene_target: &wgpu::TextureView = if hdr_active {
            hdr_view.as_ref().expect("HDR view checked above")
        } else {
            draw_target
        };
        let color_target = if hdr_active {
            ColorTarget::Hdr
        } else {
            ColorTarget::Ldr
        };
        // P3-R211：封口要不要把单面网格也切成双面变体——只有本帧真挂了会话裁剪平面才算。
        // 判据和 `mesh3d_pbr.wgsl` 的裁剪分支同一口径（法线模长平方 > 1e-8），否则会出现
        // 「换了管线却没有切面」的空付背面。Legacy 着色下选键会自己把双面轴归零（P3-R02-4），
        // 所以 pbr=off 时这条档也是空操作。
        let cutaway_both_sides = self.clip_cap_both_sides
            && scene3d.is_some_and(|scene| {
                let clip = scene.camera.clip_plane;
                let normal_sq = clip[0] * clip[0] + clip[1] * clip[1] + clip[2] * clip[2];
                normal_sq > 1e-8
            });
        // P3-R02-4 / P3-R03-2：只为本帧真正出现的 alpha/双面/IBL 组合补编译管线（每个组合一次）。
        self.ensure_frame_pipelines(&frame_draws, shading, color_target, ibl, cutaway_both_sides);
        self.write_environment_uniform(ibl);
        // docs/54 §4.3：打开后处理后场景先画进**离屏**附件，那张附件上没有"上一帧留下的内容"
        // 可叠；HDR 目标同理（未覆盖区域会是上一帧的残留）。这里显式改成历史默认清屏色并告警
        // ——静默把 `None` 当 `Color(默认)` 会让"我声明了不清屏却没生效"变成无从查起的问题。
        let clear = match clear {
            None if post_process.is_some() || hdr_active => {
                tracing::warn!(
                    target: "render",
                    "[runtime] post-process/HDR 需要确定的基色：本帧的 ClearPolicy::None 按默认清屏色执行"
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
        metrics.hdr_active = hdr_active;
        metrics.tonemap = hdr_active.then(|| post_process.unwrap_or_default().tonemap);
        let post = post_process.unwrap_or_default();
        metrics.bloom_active = hdr_active && (post.bloom > 0.0 || self.render_settings.bloom);
        metrics.fxaa_active = hdr_active && self.render_settings.fxaa;
        metrics.ssao_active = hdr_active && self.render_settings.ssao;
        metrics.ssr_active = hdr_active && self.render_settings.ssr;
        metrics.contact_shadows_active = hdr_active && self.render_settings.contact_shadows;
        metrics.fog_active = hdr_active
            && self.render_settings.fog
            && self.render_settings.fog_density.is_finite()
            && self.render_settings.fog_density > 0.0;
        // 天空与距离雾同一开关：HDR + 雾 + 有场景相机时，Tonemap 把 depth=1 的
        // 背景替换成程序化天空（地平线/天顶/太阳盘）；没有相机时保持清屏色。
        metrics.sky_active = metrics.fog_active && scene3d.is_some();
        metrics.god_rays_active = hdr_active && self.render_settings.god_rays;
        metrics.dof_active = hdr_active && self.render_settings.dof;
        metrics.soft_shadows_active = self.render_settings.soft_shadows;
        metrics.scene_format = if hdr_active {
            SceneColorFormat::Rgba16Float
        } else if self.config.format.is_srgb() {
            SceneColorFormat::Rgba8UnormSrgb
        } else {
            SceneColorFormat::Rgba8Unorm
        };
        // P3-R170：平面反射只在 HDR + PBR + SSR + 水面 + 有网格时出现；水的平面直接取
        // 第一块 `__water__`（与 Tonemap 用的是同一份数据，两处不能各自解析）。
        let water_plane = scene3d.and_then(|scene| water_plane_from_drawables(&scene.drawables));
        let planar_reflection = matches!(shading, Shading::Pbr)
            && hdr_active
            && self.render_settings.ssr
            && water_plane.is_some()
            && !frame_draws.is_empty();
        // docs/54 §4.3：本帧跑哪些通道由**计划**决定，而不是散在下面的 `if` 里。
        // 计划是纯函数（`RenderPlan::for_frame`）⇒ 顺序与取舍可以在不启动 GPU 的单测里断言。
        let plan = RenderPlan::for_frame(FrameFacts {
            clear: clear.is_some(),
            has_3d_draws: !frame_draws.is_empty(),
            planar_reflection,
            world_sprites: mesh.world_sprites.len(),
            world_sprite_opaque_segment: mesh
                .world_sprite_draws
                .iter()
                .any(|draw| draw.depth_write),
            world_sprite_shared_depth: world_view.is_some_and(|world| world.shared_depth),
            gpu_particles: particles_active && world_view.is_some(),
            world_2d_draws: count_layer_draws(mesh, RenderLayer::World2D),
            overlay_2d_draws: count_layer_draws(mesh, RenderLayer::Overlay2D),
            screen_ui_draws: count_layer_draws(mesh, RenderLayer::ScreenUI),
            vector_text: !mesh.vector_vertices.is_empty() && self.storage_capable,
            tonemap: hdr_active,
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
            pass_timestamps.begin(encoder);
            let clear_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_target,
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
            drop(clear_pass);
            pass_timestamps.end(encoder, RenderPass::Clear);
            executed.push(RenderPass::Clear);
        }
        // 深度附件是否已被本帧某个 pass 清过。共用深度时"谁先画谁清"、后面的 pass 只 Load；
        // 否则精灵与网格的深度就不是同一个空间里的值。
        let mut depth_cleared = false;
        // ① 不透明世界 Sprite：在网格 pass **之前**写深度 ⇒ 不透明精灵可以遮挡网格。
        if plan.contains(RenderPass::WorldSpriteOpaque)
            && let Some(world) = world_view.filter(|world| world.shared_depth)
        {
            pass_timestamps.begin(encoder);
            self.draw_world_sprites(
                encoder,
                scene_target,
                target,
                world,
                DepthPass::WorldSpriteOpaque,
                hdr_active,
                &mesh.world_sprite_draws,
                &mesh.paths,
                &mesh.path_handles,
                SpritePassFilter::Opaque,
                true,
                &mut metrics,
            );
            pass_timestamps.end(encoder, RenderPass::WorldSpriteOpaque);
            depth_cleared = true;
            executed.push(RenderPass::WorldSpriteOpaque);
        }
        // P0-①：本帧的材质 bind group 在进入任何 pass 前一次性建好（阴影与网格共用同一份），
        // 环形缓冲先复位再填。此前是"每 Draw Call 每 Pass"分配 GPU buffer + bind group。
        // P3-R02：法线矩阵只在 PBR 网格通道需要；旧路径不写这份数据、不占顶点槽。
        let want_normals = shading == Shading::Pbr && plan.contains(RenderPass::DepthOpaque);
        // P3-R251：第三条并列表是"压缩档的 indirect 参数缓冲"（`Some` = 该 draw 的实例
        // 已压进前缀，按参数里的存活数绘制；`None` = 沿用 `draw.instance_count()`）。
        let (material_binds, normal_slices, indirect_args): (
            Vec<wgpu::BindGroup>,
            Vec<Option<super::arena::NormalMatrixSlice>>,
            Vec<Option<wgpu::Buffer>>,
        ) = if !frame_draws.is_empty()
            && (plan.contains(RenderPass::Shadow) || plan.contains(RenderPass::DepthOpaque))
        {
            self.material_arena_used = 0;
            self.instance_arena_used = 0;
            self.material_bind_created = 0;
            self.material_bind_reused = 0;
            let mut material_binds = Vec::with_capacity(frame_draws.len());
            let mut normal_slices = Vec::with_capacity(frame_draws.len());
            let mut indirect_args = Vec::with_capacity(frame_draws.len());
            for (index, draw) in frame_draws.iter().enumerate() {
                let (bind, slice, args) =
                    self.material_bind_group_for_draw(index, draw, want_normals);
                material_binds.push(bind);
                normal_slices.push(slice);
                indirect_args.push(args);
            }
            self.note_material_bind_frame(frame_draws.len() as u64);
            (material_binds, normal_slices, indirect_args)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        if plan.contains(RenderPass::Shadow) {
            pass_timestamps.begin(encoder);
            // P3-R180：阴影链是 pass 数的最大乘数（4 级联 + 4 ESM + 最多 8×6 点光面，每面把
            // 全部 drawable 重画一遍）。先把它的录制 CPU 单列，再决定砍哪一刀。
            let shadow_started = Instant::now();
            // P3-R189：方向光逐级联剔除。级联盒 = 阴影相机的裁剪体，盒外几何 GPU 本来
            // 也会按 z/xy 裁掉 ⇒ 按组包围球提前跳过整组，像素不变。矩阵与 uniform 同源。
            let cascade_frusta = scene3d.map(|scene| {
                let distance = (scene.camera.position - scene.camera.target).length();
                shadow_view_projections(
                    scene.light,
                    scene.camera.target,
                    distance,
                    self.shadow_resolution,
                )
                .map(Frustum::from_view_projection)
            });
            for cascade in 0..self.shadow_cascade_limit as usize {
                let mut shadow_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("rustlua-shadow-pass"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.shadow_views[cascade],
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                shadow_pass.set_bind_group(0, &self.shadow_pass_bind_groups[cascade], &[]);
                record_set_pass(&mut metrics);
                metrics.shadow_passes_recorded += 1;
                let mut bound_shadow_pipeline = None;
                for (index, draw) in frame_draws.iter().enumerate() {
                    // blend 材质不投影；mask 走带 alpha 剔除的阴影变体，双面走独立剔除状态。
                    let (key, skip) = if shading == Shading::Pbr {
                        (
                            ShaderVariantKey::shadow_with_alpha(
                                SkinMode::from(draw.material.skinned),
                                draw.material.alpha_mode,
                                draw.material.double_sided,
                            ),
                            draw.material.alpha_mode == MaterialAlphaMode::Blend,
                        )
                    } else {
                        (
                            ShaderVariantKey::shadow(SkinMode::from(draw.material.skinned)),
                            false,
                        )
                    };
                    if skip {
                        continue;
                    }
                    if let Some(frusta) = cascade_frusta
                        && !frustum_may_contain(frusta[cascade], &draw.bounds)
                    {
                        metrics.shadow_cascade_draws_culled += 1;
                        continue;
                    }
                    // P3-R248：分类排除档只掐阴影链（主视图/平面反射照旧）。放在视锥剔除
                    // **之后**：只有"本来会提交"的那几条才计入 skipped，于是
                    // `提交 + 剔除 + 排除 = drawable × 级联` 仍然可对账。单独计数——
                    // 这是观感取舍，不混进"视锥剔除只省不丢"的分子。
                    if !draw.cast_shadow {
                        metrics.shadow_draws_skipped += 1;
                        continue;
                    }
                    // P3-R256：距离预算档——按包围盒到相机的**最近**距离判（与
                    // `max_draw_distance` 同形），远处 caster 不进阴影链，主视图/planar 照旧。
                    // 放在分类排除之后：两档可叠加，计数分开，守恒式仍按"本应提交"计。
                    if shadow_caster_budget > 0.0
                        && point_to_aabb_distance_sq(&draw.bounds, shadow_caster_camera)
                            > shadow_caster_budget_sq
                    {
                        metrics.shadow_distance_skipped += 1;
                        continue;
                    }
                    // P3-R247：只有真录进去的才算提交——与上面的剔除计数同帧可对账。
                    metrics.shadow_draws_drawn += 1;
                    metrics.shadow_instances_drawn += draw.instance_count() as u64;
                    if bound_shadow_pipeline != Some(key) {
                        let pipeline = self.pipelines.get(&key).unwrap_or_else(|| {
                            panic!("阴影管线变体未在构造期预编译：{}", key.label())
                        });
                        shadow_pass.set_pipeline(pipeline.as_ref());
                        bound_shadow_pipeline = Some(key);
                    }
                    shadow_pass.set_bind_group(1, &material_binds[index], &[]);
                    shadow_pass.set_vertex_buffer(0, draw.geometry.vertex_buffer.slice(..));
                    shadow_pass.set_index_buffer(
                        draw.geometry.index_buffer.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    match &indirect_args[index] {
                        Some(args) => shadow_pass.draw_indexed_indirect(args, 0),
                        None => shadow_pass.draw_indexed(
                            0..draw.geometry.index_count,
                            0,
                            0..draw.instance_count() as u32,
                        ),
                    }
                }
                drop(shadow_pass);
            }
            if self.render_settings.filtered_shadows {
                for cascade in 0..self.shadow_cascade_limit as usize {
                    let mut esm_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("rustlua-esm-shadow-pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.esm_layer_views[cascade],
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
                    esm_pass.set_pipeline(&self.esm_pipeline);
                    esm_pass.set_bind_group(0, &self.esm_bind_groups[cascade], &[]);
                    metrics.shadow_passes_recorded += 1;
                    esm_pass.draw(0..3, 0..1);
                }
                metrics.filtered_shadows_active = true;
            }
            metrics.record_shadow_us += shadow_started.elapsed().as_micros() as u64;
            let point_shadow_started = Instant::now();
            // 前八盏（最多 8 盏）点光：每盏 6 个立方体面各一次深度 pass；主着色器按光源位置找槽位、
            // 主着色器按 cube array 方向交给硬件跨面过滤。
            // P3-R202：盏数预算——只在预算内给最近的 N 盏点光录阴影；选择结果与
            // write_light_uniforms 同源（同函数、同排序键），槽位与 uniform 一一对应。
            let point_shadow_selection: Vec<usize> = if self.render_settings.point_shadows {
                scene3d
                    .map(|scene| {
                        point_shadow_light_indices(scene, self.point_shadow_light_budget as usize)
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let point_shadow_active = !point_shadow_selection.is_empty();
            metrics.point_shadows_active = point_shadow_active;
            if point_shadow_active {
                let point_lights = scene3d
                    .map(|scene| scene.point_lights.as_slice())
                    .unwrap_or(&[]);
                // P3-R194：每盏灯六个面的视锥一次建好——面视锥 = 该面相机的裁剪体，
                // 盒外几何 GPU 本来也会裁掉 ⇒ 逐面剔除像素不变。
                let face_frusta: Vec<[Frustum; 6]> = point_shadow_selection
                    .iter()
                    .map(|&index| {
                        let light = &point_lights[index];
                        point_shadow_view_projections(light.position, point_shadow_far(light.range))
                            .map(Frustum::from_view_projection)
                    })
                    .collect();
                for layer in 0..point_shadow_selection.len() * 6 {
                    // P3-R188：range 外的投射者不可能遮挡 range 内的接收者（比它更近的
                    // 遮挡体也必在 range 内）⇒ 按组包围盒到灯的距离整组剔除，像素不变。
                    let light = &point_lights[point_shadow_selection[layer / 6]];
                    let range_sq = light.range.max(1e-4) * light.range.max(1e-4);
                    let mut point_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("rustlua-point-shadow-pass"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &self.point_shadow_face_views[layer],
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    point_pass.set_bind_group(0, &self.point_shadow_bind_groups[layer], &[]);
                    record_set_pass(&mut metrics);
                    metrics.point_shadow_passes_recorded += 1;
                    let mut bound_shadow_pipeline = None;
                    for (index, draw) in frame_draws.iter().enumerate() {
                        let (key, skip) = if shading == Shading::Pbr {
                            (
                                ShaderVariantKey::shadow_with_alpha(
                                    SkinMode::from(draw.material.skinned),
                                    draw.material.alpha_mode,
                                    draw.material.double_sided,
                                ),
                                draw.material.alpha_mode == MaterialAlphaMode::Blend,
                            )
                        } else {
                            (
                                ShaderVariantKey::shadow(SkinMode::from(draw.material.skinned)),
                                false,
                            )
                        };
                        if skip {
                            continue;
                        }
                        if point_to_aabb_distance_sq(&draw.bounds, light.position) >= range_sq {
                            metrics.point_shadow_draws_culled += 1;
                            continue;
                        }
                        if self.point_face_cull_enabled
                            && !frustum_may_contain(face_frusta[layer / 6][layer % 6], &draw.bounds)
                        {
                            metrics.point_shadow_face_draws_culled += 1;
                            continue;
                        }
                        // P3-R249：分类排除档同样掐点光链（主视图/planar 照旧）。位置在
                        // range 与面剔除之后 ⇒ 只有"本来会提交"的那几条计入；单独计数，
                        // 不与方向光的 `shadow_draws_skipped` 混账。
                        // P3-R265：改读点光专用位——`shadow_caster_skip` 两链一起掐，
                        // `point_shadow_caster_skip` 只清这一位、CSM 的 crowd 阴影保留。
                        if !draw.cast_point_shadow {
                            metrics.point_shadow_draws_skipped += 1;
                            continue;
                        }
                        // P3-R256：距离预算同样掐点光链（跨灯 × 面累计）。
                        if shadow_caster_budget > 0.0
                            && point_to_aabb_distance_sq(&draw.bounds, shadow_caster_camera)
                                > shadow_caster_budget_sq
                        {
                            metrics.point_shadow_distance_skipped += 1;
                            continue;
                        }
                        if bound_shadow_pipeline != Some(key) {
                            let pipeline = self.pipelines.get(&key).unwrap_or_else(|| {
                                panic!("阴影管线变体未在构造期预编译：{}", key.label())
                            });
                            point_pass.set_pipeline(pipeline.as_ref());
                            bound_shadow_pipeline = Some(key);
                        }
                        point_pass.set_bind_group(1, &material_binds[index], &[]);
                        point_pass.set_vertex_buffer(0, draw.geometry.vertex_buffer.slice(..));
                        point_pass.set_index_buffer(
                            draw.geometry.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        // P3-R261：点光链的提交也要进读数——§21.540 的「预算砍了 CSM 实例、
                        // shadow 时间不动」就是因为这条链从来不记 draw/实例。
                        metrics.point_shadow_draws_drawn += 1;
                        metrics.point_shadow_instances_drawn += draw.instance_count() as u64;
                        match &indirect_args[index] {
                            Some(args) => point_pass.draw_indexed_indirect(args, 0),
                            None => point_pass.draw_indexed(
                                0..draw.geometry.index_count,
                                0,
                                0..draw.instance_count() as u32,
                            ),
                        }
                    }
                    drop(point_pass);
                }
            }
            metrics.record_point_shadow_us += point_shadow_started.elapsed().as_micros() as u64;
            pass_timestamps.end(encoder, RenderPass::Shadow);
            executed.push(RenderPass::Shadow);
        }
        // P3-R170：水面平面反射。镜像相机把不透明网格再画一遍到半分辨率色图，Tonemap
        // 在水面像素上按投影 uv 采样。水面本身与 Blend 材质跳过（前者是镜面，后者会
        // 在镜像里留下半透明残影）。
        //
        // 两条与主通道不同的规则：
        // ① **双面管线**：镜像是保长反射（det = -1），同一三角形的屏幕绕序与主通道相反，
        //    单面管线会把它整片剔除（实测覆盖 0；双面 244 像素）。主通道的 alpha/skin/IBL
        //    档不变，只在构造期多预热一份双面变体。
        // ② **水面高度裁剪面**：`y >= level` 之外（水下的河床、埋在水里的几何）不属于
        //    反射内容，交给着色器的世界裁剪分支丢弃；否则它们会被镜像到水面之上。
        if plan.contains(RenderPass::PlanarReflection)
            && let (Some(scene), Some(plane)) = (scene3d, water_plane)
            && let Some(matrix) = main_matrix.or_else(|| scene.view.matrix())
        {
            self.ensure_planar_reflection_target(target.0, target.1);
            let reflected = super::planar_reflection_view_projection(&matrix, plane[0]);
            let mirrored_position = Vec3::new(
                scene.camera.position.x,
                2.0 * plane[0] - scene.camera.position.y,
                scene.camera.position.z,
            );
            let uniform = super::camera_uniform(
                &reflected,
                mirrored_position,
                self.clock_secs(),
                // 反射内容只在水平面之上；这条裁剪面同时替宿主省掉水下几何的填充。
                [0.0, 1.0, 0.0, -plane[0]],
                self.cloud_coverage,
                self.cloud_speed,
                self.cloud_base,
                self.cloud_shadow_strength,
                self.water_tint,
                self.surface_cover,
                [0.0, 0.0],
                [0.0; 4],
                // P3-R209：反射 pass 的这条裁剪面是**水位**，不是会话剖面。封口深度写死 0
                // ⇒ 水下几何照旧只是被丢弃，不会被当成实体土壤反射出来。
                0.0,
            );
            self.queue.write_buffer(
                &self.planar_camera_buffer,
                0,
                bytemuck::cast_slice(&uniform),
            );
            pass_timestamps.begin(encoder);
            let mut planar_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-planar-reflection-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.planar_reflection_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // 没被任何网格覆盖的像素保持全透明 = "这里没有反射"，Tonemap
                        // 用 alpha 掩码回退到屏幕空间反射。
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.planar_reflection_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            planar_pass.set_bind_group(0, &self.planar_camera_bind_group, &[]);
            if shading == Shading::Pbr {
                planar_pass.set_bind_group(2, &self.environment_bind_group, &[]);
            }
            record_set_pass(&mut metrics);
            let mut bound_pipeline = None;
            // P3-R191：镜像相机视锥外的几何不进反射图（镜像图只由这些像素组成），
            // 逐组剔除零视觉变化——与阴影级联同一套保守判定。
            let planar_frustum = Frustum::from_view_projection(reflected);
            for (index, draw) in frame_draws.iter().enumerate() {
                if draw.water || draw.material.alpha_mode == MaterialAlphaMode::Blend {
                    continue;
                }
                if self.planar_cull_enabled && !frustum_may_contain(planar_frustum, &draw.bounds) {
                    metrics.planar_draws_culled += 1;
                    continue;
                }
                let key = ShaderVariantKey::mesh_with_alpha(
                    shading,
                    SkinMode::from(draw.material.skinned),
                    ColorTarget::Hdr,
                    draw.material.alpha_mode,
                    true,
                )
                .with_ibl(ibl);
                if bound_pipeline != Some(key) {
                    let pipeline = self.pipelines.get(&key).unwrap_or_else(|| {
                        panic!("平面反射管线变体未在构造期预编译：{}", key.label())
                    });
                    planar_pass.set_pipeline(pipeline.as_ref());
                    bound_pipeline = Some(key);
                }
                planar_pass.set_bind_group(1, &material_binds[index], &[]);
                planar_pass.set_vertex_buffer(0, draw.geometry.vertex_buffer.slice(..));
                if shading == Shading::Pbr
                    && let Some(normals) = normal_slices[index].as_ref()
                {
                    planar_pass.set_vertex_buffer(
                        1,
                        normals
                            .buffer
                            .slice(normals.offset..normals.offset + normals.size),
                    );
                }
                planar_pass.set_index_buffer(
                    draw.geometry.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                match &indirect_args[index] {
                    Some(args) => planar_pass.draw_indexed_indirect(args, 0),
                    None => planar_pass.draw_indexed(
                        0..draw.geometry.index_count,
                        0,
                        0..draw.instance_count() as u32,
                    ),
                }
                metrics.planar_reflection_draws += 1;
            }
            drop(planar_pass);
            pass_timestamps.end(encoder, RenderPass::PlanarReflection);
            metrics.planar_reflection_active = true;
            executed.push(RenderPass::PlanarReflection);
        }
        if plan.contains(RenderPass::DepthOpaque) {
            self.ssao_depth_view = self.depth_view.clone();
            pass_timestamps.begin(encoder);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-mesh3d-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_target,
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
            record_set_pass(&mut metrics);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            // P3-R03：只有 PBR 管线布局含环境组；Legacy 布局两组，绑第三组会触发校验错误。
            if shading == Shading::Pbr {
                pass.set_bind_group(2, &self.environment_bind_group, &[]);
            }
            apply_declared_viewport(&mut pass, main_viewport, target);
            let mut bound_mesh_pipeline = None;
            for (index, draw) in frame_draws.iter().enumerate() {
                // P3-R211：主通道的双面轴必须和 `ensure_frame_pipelines` 的选键同式
                // （材质自身 || 本帧封口强制），少一边就会撞上未预编译的 panic。
                let key = ShaderVariantKey::mesh_with_alpha(
                    shading,
                    SkinMode::from(draw.material.skinned),
                    color_target,
                    draw.material.alpha_mode,
                    draw.material.double_sided || cutaway_both_sides,
                )
                .with_ibl(ibl);
                if bound_mesh_pipeline != Some(key) {
                    let pipeline = self.pipelines.get(&key).unwrap_or_else(|| {
                        panic!("3D 网格管线变体未在构造期预编译：{}", key.label())
                    });
                    pass.set_pipeline(pipeline.as_ref());
                    bound_mesh_pipeline = Some(key);
                }
                pass.set_bind_group(1, &material_binds[index], &[]);
                pass.set_vertex_buffer(0, draw.geometry.vertex_buffer.slice(..));
                if shading == Shading::Pbr {
                    let normals = normal_slices[index]
                        .as_ref()
                        .expect("PBR 网格通道缺少逐实例法线矩阵");
                    pass.set_vertex_buffer(
                        1,
                        normals
                            .buffer
                            .slice(normals.offset..normals.offset + normals.size),
                    );
                }
                pass.set_index_buffer(
                    draw.geometry.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                match &indirect_args[index] {
                    Some(args) => pass.draw_indexed_indirect(args, 0),
                    None => pass.draw_indexed(
                        0..draw.geometry.index_count,
                        0,
                        0..draw.instance_count() as u32,
                    ),
                }
            }
            drop(pass);
            pass_timestamps.end(encoder, RenderPass::DepthOpaque);
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
            let clear_world_depth = !depth_cleared;
            pass_timestamps.begin(encoder);
            self.draw_world_sprites(
                encoder,
                scene_target,
                target,
                world,
                DepthPass::WorldSprite,
                hdr_active,
                &mesh.world_sprite_draws,
                &mesh.paths,
                &mesh.path_handles,
                filter,
                clear_world_depth,
                &mut metrics,
            );
            pass_timestamps.end(encoder, RenderPass::WorldSprite);
            if clear_world_depth {
                depth_cleared = true;
            }
            executed.push(RenderPass::WorldSprite);
        }
        // M74-03 P2：GPU 粒子。compute 已在 `step_gpu_particles` 里提交（本帧 render 之前），
        // 这里只把顶点缓冲画出来——渲染侧零回读、零上传。
        if plan.contains(RenderPass::Particle)
            && let Some(world) = world_view
        {
            pass_timestamps.begin(encoder);
            if self.draw_gpu_particles(
                encoder,
                scene_target,
                target,
                world,
                hdr_active,
                !depth_cleared,
                &mut metrics,
            ) {
                pass_timestamps.end(encoder, RenderPass::Particle);
                executed.push(RenderPass::Particle);
            } else {
                pass_timestamps.abandon();
            }
        }
        if self.render_settings.ssr
            && let Some(scene) = scene3d
            && let Some(source) = hdr_view.as_ref()
        {
            metrics.ssr_active = self.draw_ssr(encoder, source, scene, target);
        }
        if self.render_settings.ssao
            && let Some(scene) = scene3d
        {
            let executed_ssao = self.draw_ssao(encoder, scene, target);
            metrics.ssao_active = self.render_settings.ssao && executed_ssao;
        }
        // P3-R01：Tonemap 必须在 3D/世界内容之后、所有 2D/UI/矢量文字之前执行，
        // 否则 ACES/Reinhard 会连带改掉 UI 颜色。HDR 关闭时计划里没有这条通道。
        if plan.contains(RenderPass::Tonemap)
            && let Some(source) = hdr_view.as_ref()
        {
            let mut settings = post_process.unwrap_or_default();
            // 场景声明 Environment 时宿主给 RenderSettings 打开泛光；显式 PostProcess
            // 已经设了非零 bloom 时尊重显式值，避免两个入口互相覆盖。
            if self.render_settings.bloom && settings.bloom == 0.0 {
                settings.bloom = 0.18;
            }
            pass_timestamps.begin(encoder);
            let vertices = self.draw_tonemap(
                encoder,
                source,
                draw_target,
                settings,
                self.render_settings.fxaa,
                self.render_settings.ssao,
                self.render_settings.ssr,
                planar_reflection,
                self.render_settings.contact_shadows,
                self.render_settings.fog,
                self.render_settings.fog_density,
                self.render_settings.fog_color,
                self.render_settings.god_rays,
                self.render_settings.dof,
                self.render_settings.dof_focus,
                self.render_settings.dof_range,
                self.render_settings.dof_strength,
                self.render_settings.motion_blur,
                scene3d,
                target,
            );
            pass_timestamps.end(encoder, RenderPass::Tonemap);
            record_set_pass(&mut metrics);
            record_triangle_draw(&mut metrics, vertices as u32);
            executed.push(RenderPass::Tonemap);
        }
        if plan.contains(RenderPass::Legacy2DOverlay) || plan.contains(RenderPass::ScreenUI) {
            let bytes = bytemuck::cast_slice(&mesh.vertices);
            // 2D 顶点缓冲按需增长。此前是固定 1 MiB + `if bytes.len() <= size`
            // **没有 else 分支**：超限时整个 2D 层（含 UI/文字）静默消失，日志里
            // 什么线索都没有。现在超限即扩容并留下可诊断的计数与日志。
            self.ensure_2d_vertex_capacity(bytes.len() as u64);
            self.queue.write_buffer(&self.vertex_buffer, 0, bytes);
            pass_timestamps.begin(encoder);
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
            record_set_pass(&mut metrics);
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
                if !plan.contains(stage) || !include_ui {
                    continue;
                }
                let mut batch: Option<OverlayBatch> = None;
                // 上面已经绑了普通管线；只有真的切到 SDF 管线（或切回来）才计一次 SetPass。
                let mut overlay_pipeline_sdf = false;
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
                            if overlay_pipeline_sdf != key.sdf {
                                pass.set_pipeline(if key.sdf {
                                    &self.pipeline_sdf
                                } else {
                                    &self.pipeline
                                });
                                record_set_pass(&mut metrics);
                                overlay_pipeline_sdf = key.sdf;
                            }
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
            drop(pass);
            // Legacy2DOverlay 与 ScreenUI 共用这一次 pass：ScreenUI 不单列，整段记在 overlay 上。
            pass_timestamps.end(encoder, RenderPass::Legacy2DOverlay);
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
            pass_timestamps.begin(encoder);
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
            record_set_pass(&mut metrics);
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, self.vector_vertex_buffer.slice(..));
            apply_declared_viewport(&mut pass, overlay_viewport, target);
            pass.draw(0..mesh.vector_vertices.len() as u32, 0..1);
            record_triangle_draw(&mut metrics, mesh.vector_vertices.len() as u32);
            drop(pass);
            pass_timestamps.end(encoder, RenderPass::VectorText);
            executed.push(RenderPass::VectorText);
        }
        // docs/54 §4.3：最后一条通道——把离屏场景颜色按曝光/色调/暗角写进**目标附件**。
        // P3-R01：HDR 帧的曝光已经由 Tonemap 消费，这里把 EV 归零，避免乘两次。
        if plan.contains(RenderPass::PostProcess)
            && let (Some(source), Some(mut settings)) = (scene_view.as_ref(), post_process)
        {
            if hdr_active {
                settings.exposure_ev = 0.0;
            }
            pass_timestamps.begin(encoder);
            let vertices = self.draw_post_process(encoder, source, view, settings, target);
            pass_timestamps.end(encoder, RenderPass::PostProcess);
            record_set_pass(&mut metrics);
            record_triangle_draw(&mut metrics, vertices as u32);
            executed.push(RenderPass::PostProcess);
        }
        // 把离屏视图放回（下一帧复用；纹理本体一直在 `self` 里，视图只是个句柄）。
        self.post_process_scene_view = scene_view;
        self.hdr_scene_view = hdr_view;
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
        let timestamp_layout = pass_timestamps.finish();
        self.timestamp_query_set = timestamp_query_set;
        (metrics, timestamp_layout)
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
    /// 第四项 `sampling_epoch` 是 M79 P1-3c 加的：改导入设置会**就地换掉某个槽位的绑定组**
    /// 而不动 `textures.len()`（`reload_import_settings`），前三项全都看不见它。少了它，
    /// "把这张图改成线性"要等下一批纹理进来才生效——表现为改了没反应。
    ///
    /// 注意这里**故意**不含 `texture_identity_hits/misses`：那两个是诊断计数，命中与否不改变结果。
    pub(super) fn texture_epoch(&self) -> (usize, usize, bool, u64) {
        (
            self.textures.len(),
            self.custom_fonts.len(),
            self.font_atlas_bind.is_some(),
            self.sampling_epoch,
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
    epoch: (usize, usize, bool, u64),
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
