//! 内嵌游戏预览会话（PreviewSession）：把 Play 预览的运行时、离屏渲染器、
//! 纹理/视图与回读上传封装为可独立创建/步进/缩放/销毁的生命周期边界。
//!
//! 职责划分：本模块持有全部 GPU/运行时资源并驱动 ~60fps 节流步进与非阻塞回读；
//! Game 视图只负责输入桥接与绘制，Scene 视图可复用 readback 生命周期辅助。

use eframe::egui;
use engine::core::input::InputSnapshot;
use engine::core::scene::Scene;
use engine::host::{ReadbackMetrics, ReadbackStatus, Renderer, RuntimeScene};
use std::path::Path;

/// 游戏预览的重绘节流：~60fps，保证持续运行而不忙转。
const PREVIEW_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// 启动期帧率打点：以 120 个到期 tick 为一窗口（tick 数、就绪上传数、窗口起始），
/// 窗口结束时记录真实显示帧率（上传次数即显示帧率依据）。仅日志证据，不持有会话状态。
static PERF_WINDOW: std::sync::Mutex<Option<(u32, u32, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// 一次 `update` 后的状态，供视图决定 overlay 与 repaint 调度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PreviewTick {
    /// 未到节流周期，仅排空在途回读（可能补上传上一滞后帧）。
    #[default]
    Idle,
    /// 已步进并渲染新帧，回读就绪并上传。
    Rendered,
    /// 已步进并渲染新帧，但回读仍在途（沿用上一帧）。
    Pending,
    /// 脚本步进或渲染失败；诊断已更新。
    Failed,
}

/// 内嵌游戏预览会话：运行时 + 离屏渲染器 + 纹理/视图 + egui 纹理。
pub(crate) struct PreviewSession {
    runtime: RuntimeScene,
    renderer: Renderer,
    render_state: Option<eframe::egui_wgpu::RenderState>,
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    preview_texture: Option<PreviewTexture>,
    width: u32,
    height: u32,
    /// 距上次步进的时间（~60fps 节流）。
    last_tick: Option<std::time::Instant>,
    /// 最近一次步进/回读的结构化诊断；Game 视图 overlay 显示。
    diagnostic: Option<String>,
}

enum PreviewTexture {
    Native(egui::TextureId),
    Managed(egui::TextureHandle),
}

impl PreviewSession {
    /// 创建预览会话：运行时（当前场景）+ 离屏渲染器 + 预览纹理。
    pub(crate) fn create(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        render_state: Option<eframe::egui_wgpu::RenderState>,
        project_root: &Path,
        scene: &Scene,
        size: (u32, u32),
    ) -> Result<Self, String> {
        tracing::info!(target: "editor", "[preview] project_root={}", project_root.display());
        let runtime = RuntimeScene::from_scene(Some(project_root), scene.clone())?;
        let renderer = Renderer::new_offscreen(
            device.clone(),
            queue.clone(),
            Some(project_root.to_path_buf()),
            false,
            false,
        )?;
        let (width, height) = size;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("p2-editor-preview"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut session = Self {
            runtime,
            renderer,
            render_state,
            texture,
            texture_view,
            preview_texture: None,
            width,
            height,
            last_tick: None,
            diagnostic: None,
        };
        session.sync_native_texture(device);
        Ok(session)
    }

    /// 当前分辨率（宽×高）。
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 当前 egui 预览纹理（就绪帧）；`None` 表示尚无回读结果。
    pub(crate) fn texture_id(&self) -> Option<egui::TextureId> {
        match &self.preview_texture {
            Some(PreviewTexture::Native(id)) => Some(*id),
            Some(PreviewTexture::Managed(texture)) => Some(texture.id()),
            None => None,
        }
    }

    /// 最近一次步进/回读的结构化诊断。
    pub(crate) fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }

    /// 回读指标（slot 数、busy/drop/rebuild 等），供视图与诊断显示。
    pub(crate) fn readback_metrics(&self) -> ReadbackMetrics {
        if self.uses_native_texture() {
            ReadbackMetrics::default()
        } else {
            self.renderer.readback_metrics()
        }
    }

    /// 每帧调用：先排空在途回读，到 ~60fps 节流周期时步进游戏并渲染提交新回读。
    /// 内部按需请求 repaint，保持游戏匀速而不忙转。
    pub(crate) fn update(&mut self, ctx: &egui::Context, input: &InputSnapshot) -> PreviewTick {
        let now = std::time::Instant::now();
        let due = self
            .last_tick
            .is_none_or(|last| now.duration_since(last) >= PREVIEW_FRAME_INTERVAL);
        if !due {
            // 未到期：仅排空在途回读（补上滞后帧），保持游戏匀速。
            let uploaded = self.drain_readback(ctx);
            if uploaded {
                self.diagnostic = None;
            }
            ctx.request_repaint_after(PREVIEW_FRAME_INTERVAL);
            return PreviewTick::Idle;
        }
        self.last_tick = Some(now);

        // 步进游戏运行时（固定 60Hz 逻辑步进；输入由调用方桥接）。
        if let Err(error) = self.runtime.run_script_steps(1, 1.0 / 60.0, input) {
            tracing::warn!(target: "editor", "[preview] script step failed: {error}");
            self.diagnostic = Some(format!("脚本步进失败：{error}"));
            ctx.request_repaint_after(PREVIEW_FRAME_INTERVAL);
            return PreviewTick::Failed;
        }
        // 构建 2D 网格并渲染到离屏纹理；失败时给出用户可见诊断而不是空白帧。
        let mesh = match self.runtime.vertices(self.width, self.height) {
            Ok(mesh) => mesh,
            Err(error) => {
                tracing::error!(target: "editor", "[preview] build vertices failed: {error}");
                self.diagnostic = Some(format!("渲染失败：{error}"));
                ctx.request_repaint_after(PREVIEW_FRAME_INTERVAL);
                return PreviewTick::Failed;
            }
        };
        let aspect = self.width as f32 / self.height.max(1) as f32;
        let scene3d = self.runtime.scene_3d(aspect);
        self.renderer.render_into_texture(
            &mesh,
            scene3d.as_ref(),
            &self.texture_view,
            self.width,
            self.height,
        );
        if self.uses_native_texture() {
            self.diagnostic = None;
            self.record_presented_frame();
            self.record_tick_window();
            ctx.request_repaint_after(PREVIEW_FRAME_INTERVAL);
            return PreviewTick::Rendered;
        }
        // 非阻塞回读：就绪则上传；未就绪沿用上一帧（主线程不阻塞）。
        let tick = match self
            .renderer
            .readback_frame(&self.texture, self.width, self.height)
        {
            ReadbackStatus::Ready {
                pixels,
                width,
                height,
            } => {
                self.upload_frame(ctx, pixels, width, height);
                self.diagnostic = None;
                PreviewTick::Rendered
            }
            ReadbackStatus::Pending => {
                self.diagnostic = Some("等待 GPU 回读……显示上一帧".to_owned());
                PreviewTick::Pending
            }
            ReadbackStatus::Failed(error) => {
                tracing::warn!(target: "editor", "[preview] readback failed: {error}");
                self.diagnostic = Some(format!("GPU 回读失败：{error}"));
                PreviewTick::Failed
            }
        };
        self.record_tick_window();
        // 节流到 ~60fps：无论回读是否就绪都排下一帧，保证游戏持续运行且不忙转。
        ctx.request_repaint_after(PREVIEW_FRAME_INTERVAL);
        tick
    }

    fn record_tick_window(&self) {
        // 启动期帧率打点：每 120 个到期 tick 记录一次真实显示帧率（上传数由 upload_frame 累计）。
        let mut window = PERF_WINDOW.lock().expect("preview perf window lock");
        let (ticks, uploads, start) =
            window.get_or_insert_with(|| (0, 0, std::time::Instant::now()));
        *ticks += 1;
        if *ticks >= 120 {
            let fps = *uploads as f64 / start.elapsed().as_secs_f64().max(0.001);
            tracing::info!(
                target: "editor",
                "[editor-perf] preview fps: {uploads} uploads / {ticks} ticks = {fps:.1} fps"
            );
            *window = None;
        }
    }

    /// 原子替换离屏纹理/视图（新尺寸），保留运行时与渲染器。
    /// 旧的读回拷贝已在 GPU 队列中引用旧纹理，wgpu 会持有资源直到提交完成，
    /// 因此替换后旧在途帧仍可安全排空（以旧尺寸就绪）。
    pub(crate) fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("p2-editor-preview"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.texture = texture;
        self.texture_view = texture_view;
        self.width = width;
        self.height = height;
        self.sync_native_texture(device);
        tracing::debug!(target: "editor", "[preview] resized {}x{}", width, height);
    }

    /// 排空在途回读并上传就绪帧；返回是否有新帧上传。
    fn drain_readback(&mut self, ctx: &egui::Context) -> bool {
        match self.renderer.readback_pending() {
            ReadbackStatus::Ready {
                pixels,
                width,
                height,
            } => {
                self.upload_frame(ctx, pixels, width, height);
                true
            }
            _ => false,
        }
    }

    /// 把回读像素上传到 egui 纹理（复用同尺寸纹理，仅 ready 时上传）。
    fn upload_frame(&mut self, ctx: &egui::Context, pixels: Vec<u8>, width: u32, height: u32) {
        let nonzero = pixels.iter().any(|&byte| byte > 0);
        if !nonzero {
            tracing::warn!(target: "editor", "[preview] readback frame is all-zero");
        }
        let image =
            egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &pixels);
        match &mut self.preview_texture {
            Some(PreviewTexture::Managed(tex)) => tex.set(image, egui::TextureOptions::LINEAR),
            None => {
                self.preview_texture = Some(PreviewTexture::Managed(ctx.load_texture(
                    "game_preview",
                    image,
                    egui::TextureOptions::LINEAR,
                )));
            }
            Some(PreviewTexture::Native(_)) => {}
        }
        self.record_presented_frame();
    }

    fn uses_native_texture(&self) -> bool {
        matches!(self.preview_texture, Some(PreviewTexture::Native(_)))
    }

    fn sync_native_texture(&mut self, device: &wgpu::Device) {
        let Some(render_state) = &self.render_state else {
            return;
        };
        let mut renderer = render_state.renderer.write();
        match &mut self.preview_texture {
            Some(PreviewTexture::Native(id)) => {
                renderer.update_egui_texture_from_wgpu_texture(
                    device,
                    &self.texture_view,
                    wgpu::FilterMode::Linear,
                    *id,
                );
            }
            Some(PreviewTexture::Managed(_)) => {}
            None => {
                let id = renderer.register_native_texture(
                    device,
                    &self.texture_view,
                    wgpu::FilterMode::Linear,
                );
                self.preview_texture = Some(PreviewTexture::Native(id));
            }
        }
    }

    fn record_presented_frame(&self) {
        let Ok(mut guard) = PERF_WINDOW.lock() else {
            return;
        };
        if let Some((_, uploads, _)) = guard.as_mut() {
            *uploads = uploads.saturating_add(1);
        }
    }

    /// 测试辅助：强制下一个 `update` 视为到节流周期（免去真实 16ms 等待）。
    #[cfg(test)]
    fn force_tick(&mut self) {
        self.last_tick = None;
    }
}

impl Drop for PreviewSession {
    fn drop(&mut self) {
        let (Some(render_state), Some(PreviewTexture::Native(id))) =
            (&self.render_state, self.preview_texture.as_ref())
        else {
            return;
        };
        render_state.renderer.write().free_texture(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::core::scene::Scene;

    /// Headless wgpu 设备/队列（fallback adapter）；无 GPU 时返回 None，测试跳过。
    fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("p2-preview-session-test"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()
    }

    /// 100 次 create → 多次 tick → resize 往返 → drop 生命周期：断言无 panic、
    /// readback slot 数保持上界、指标已提交、CPU 缓冲有界。
    #[test]
    fn preview_session_100_create_tick_resize_drop() {
        let Some((device, queue)) = test_device() else {
            return;
        };
        let ctx = egui::Context::default();
        let scene = Scene::new("lifecycle");
        let sizes = [(64, 64), (640, 360), (1920, 1080)];
        for cycle in 0..100 {
            let (width, height) = sizes[cycle % sizes.len()];
            let mut session = PreviewSession::create(
                &device,
                &queue,
                None,
                Path::new("."),
                &scene,
                (width, height),
            )
            .expect("create preview session");
            assert_eq!(session.size(), (width, height));
            for _ in 0..4 {
                session.force_tick();
                session.update(&ctx, &InputSnapshot::default());
            }
            // 尺寸往返切换：原子替换纹理，旧在途回读安全排空。
            session.resize(&device, height.max(16), width.max(16));
            session.resize(&device, width, height);
            session.update(&ctx, &InputSnapshot::default());
            let metrics = session.readback_metrics();
            assert!(metrics.slot_count <= 2, "readback slot 数应保持上界");
            assert!(
                metrics.buffer_rebuilds <= 4,
                "buffer 重建应有上界: {}",
                metrics.buffer_rebuilds
            );
            assert!(metrics.submitted >= 1, "应至少提交过一次回读");
            // drop 释放 GPU 纹理/视图/渲染器。
        }
    }

    /// 单会话连续 update：到期步进并返回 Rendered/Pending，紧随其后的 update
    /// 未到期 → Idle（仅排空在途回读）；指标单调不倒退。
    #[test]
    fn preview_session_tick_returns_rendered_or_pending() {
        let Some((device, queue)) = test_device() else {
            return;
        };
        let ctx = egui::Context::default();
        let scene = Scene::new("tick");
        let mut session =
            PreviewSession::create(&device, &queue, None, Path::new("."), &scene, (64, 64))
                .expect("create preview session");
        session.force_tick();
        let first = session.update(&ctx, &InputSnapshot::default());
        assert!(matches!(
            first,
            PreviewTick::Pending | PreviewTick::Rendered
        ));
        // 紧随其后的 update 未到期 → Idle（仅排空在途回读）。
        let idle = session.update(&ctx, &InputSnapshot::default());
        assert_eq!(idle, PreviewTick::Idle);
        // 二次强制到期：应能排空上一回读并再次渲染。
        session.force_tick();
        let second = session.update(&ctx, &InputSnapshot::default());
        assert!(matches!(
            second,
            PreviewTick::Pending | PreviewTick::Rendered
        ));
        // 指标单调不倒退。
        let before = session.readback_metrics();
        session.force_tick();
        session.update(&ctx, &InputSnapshot::default());
        let after = session.readback_metrics();
        assert!(after.submitted >= before.submitted);
        assert!(after.completed >= before.completed);
    }

    /// resize 幂等（同尺寸无操作），且已上传帧后 resize 复用同尺寸 egui 纹理句柄。
    #[test]
    fn preview_session_resize_is_idempotent_and_reuses_egui_texture() {
        let Some((device, queue)) = test_device() else {
            return;
        };
        let ctx = egui::Context::default();
        let scene = Scene::new("resize");
        let mut session =
            PreviewSession::create(&device, &queue, None, Path::new("."), &scene, (320, 180))
                .expect("create preview session");
        // 驱动多帧让 egui 纹理存在（回读异步，故循环强制到期）。
        for _ in 0..8 {
            session.force_tick();
            session.update(&ctx, &InputSnapshot::default());
        }
        let Some(id_before) = session.texture_id() else {
            // 回读尚未就绪时跳过句柄断言（环境允许异步延迟）。
            return;
        };
        session.resize(&device, 320, 180); // 同尺寸 → 无操作。
        assert_eq!(session.size(), (320, 180));
        session.resize(&device, 640, 360);
        assert_eq!(session.size(), (640, 360));
        session.resize(&device, 640, 360); // 幂等。
        assert_eq!(session.size(), (640, 360));
        let id_after = session.texture_id();
        assert_eq!(Some(id_before), id_after, "resize 应复用 egui 纹理句柄");
    }
}
