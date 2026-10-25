//! 渲染管线：2D/3D/游戏画面三条离屏渲染路径，纹理同步、回读与资源释放。

use super::*;

impl EditorApp {
    /// 项目根变化时销毁携带旧 root 的 GPU 渲染器。Renderer 会把相对纹理路径
    /// 解析成 `root + path`；跨项目复用旧实例会让 Scene View 加载上一个项目的
    /// 同名资源（Play 预览每次新建 renderer，因此不受影响）。
    pub(crate) fn reset_scene_render_resources(&mut self) {
        self.scene_renderer = None;
        self.scene_texture = None;
        // 离屏纹理没了，2D/3D 注册的原生 egui 纹理 ID 与「纹理里画的是什么」的渲染
        // 缓存必须一起失效：项目切换后新旧文档 revision 都可能是 0，缓存误命中就会
        // 跳过重渲染，视图停在空白画面。
        self.release_scene_view_textures();
        self.scene_view_cache_2d = SceneViewCache2d::default();
        self.scene_view_cache_3d = SceneViewCache3d::default();
        self.scene_screen_texture = None;
        self.scene_screen_tex_id = None;
        self.scene_screen_cache_key = None;
        // 失败去抖是按文本记的：不清掉，切到另一个项目里**同名同错**的坏 shader 就一句都不播了。
        self.shader_error_shown = None;
        self.shader_watch_due = None;
    }

    /// 确保离屏渲染器**实例**存在；不建视图纹理（Game 视图的编辑预览只要渲染器）。
    pub(crate) fn ensure_scene_renderer_instance(&mut self) {
        if self.scene_renderer.is_some() {
            return;
        }
        let Some((device, queue)) = &self.gpu else {
            return;
        };
        let root = strip_verbatim_prefix(&self.project.root);
        self.scene_renderer =
            Renderer::new_offscreen(device.clone(), queue.clone(), Some(root), false, false).ok();
        // P3-R05-4：新建的渲染器立刻接上覆盖目录，Scene View 与 Play 读同一份源。
        // 目录为空/不存在时 `rebuilt = 0`（与内置源逐字节相同，一次重编都不做）。
        self.attach_scene_shader_overrides();
        // 运行时可能先于渲染器建立（2D 视图先建运行时）；渲染器一就绪就把场景环境
        // 上传一次，避免 Scene 3D 首帧缺环境。
        self.apply_scene_environment_settings();
        self.sync_particle_compute();
    }

    /// 确保 3D 场景视图的离屏渲染器存在；按视图尺寸重建纹理（M10-04）。
    pub(crate) fn ensure_scene_renderer(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        self.ensure_scene_renderer_instance();
        let size_changed = self
            .scene_texture
            .as_ref()
            .is_none_or(|(texture, _)| texture.width() != width || texture.height() != height);
        if size_changed {
            // 纹理换了新的：旧的原生 egui 纹理绑定组指向已废弃纹理，先释放 ID，
            // 下次 `sync_scene_view_texture` 再按新视图注册。
            self.release_scene_view_textures();
            if let Some((device, _)) = &self.gpu {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("rustlua-scene-view"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    // TEXTURE_BINDING：让 egui 直接采样这张纹理（原生路径的前提），
                    // COPY_SRC 保留给无窗口环境的回读回退。
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                self.scene_texture = Some((texture, view));
            }
        }
    }

    /// 从当前编辑场景 + 编辑器轨道相机构建 3D 渲染帧（M10-04）。
    ///
    /// M11-04 / R-05：优先复用 `RuntimeScene` 的 ECS 提取（与运行时同一条
    /// drawable/component 提取路径，实体/资源数量与关键状态一致），编辑器轨道
    /// 相机作为覆盖传入（不修改场景数据）；`scene_runtime` 缺失、实例化失败或
    /// 提取为空时回退到静态 serialized extractor（保持既有诊断行为）。
    ///
    /// 拆分批 7：参数是**视口**而不是裸比例——面板比例只经由视口进入烘焙的视图，
    /// 既不写进组件、也不需要调用方自己算一次除法（`Viewport::aspect` 是唯一推导处）。
    pub(crate) fn build_scene3d(&mut self, viewport: Viewport) -> Option<Scene3d> {
        let camera = self.editor_camera();
        if let Some(runtime) = &self.scene_runtime
            && let Some(mut scene3d) = runtime.scene_3d_with_camera(viewport, Some(camera))
        {
            // Gizmo/拖拽预览覆盖 transform（不修改场景数据、不写回 ECS）。
            for drawable in &mut scene3d.drawables {
                if let Some(t) = self.scene_state.preview3d_transform.get(&drawable.entity)
                    && let Ok(model) = t.model_matrix()
                {
                    drawable.model = model;
                }
            }
            return Some(scene3d);
        }
        // 回退：typed 提取缓存（无 ECS 场景 / 实例化失败时保持既有行为与诊断）。
        // 无场景时缓存为空，最终由下面的 `drawables.is_empty()` 统一返回 None——
        // 不必在这里提前取 `current_scene()`，那会让不可变借用跨过下面的可变调用。
        self.ensure_scene_view_3d_typed();
        if !self.scene_view_3d_typed.diagnostics.is_empty() {
            let details = self
                .scene_view_3d_typed
                .diagnostics
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>();
            let count = self.scene_view_3d_typed.diagnostics.len();
            self.set_scene_view_diagnostic(format!(
                "{count} 个 3D 组件无效：{}",
                details.join("；")
            ));
        }
        let preview = &self.scene_state.preview3d_transform;
        let drawables = self
            .scene_view_3d_typed
            .entities
            .iter()
            .filter_map(|entity| {
                let transform = preview.get(&entity.id).copied().unwrap_or(entity.transform);
                let model = transform.model_matrix().ok()?;
                Some(Scene3dDrawable {
                    entity: entity.id,
                    name: entity.name.clone(),
                    mesh_path: entity.mesh_path.clone(),
                    // typed 预览缓存只提取网格路径；主路径（`scene_3d_with_camera`）已带材质覆盖。
                    material: None,
                    // 编辑器预览自己拼 drawable（没有运行时身份表）：渲染器按路径解析。
                    mesh_handle: None,
                    model,
                    palette: Vec::new(),
                    bounds: Scene3dBounds {
                        local_aabb: rustlua_math::math3d::Aabb3 {
                            min: Vec3::new(-0.5, -0.5, -0.5),
                            max: Vec3::new(0.5, 0.5, 0.5),
                        },
                        source: BoundsSource::Fallback,
                    },
                })
            })
            .collect::<Vec<_>>();
        if drawables.is_empty() {
            return None;
        }
        let light = Light {
            direction: Vec3::new(0.4, -0.8, 0.5),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
        };
        Some(Scene3d {
            camera,
            // 编辑器这条回退路径自己拼 drawable，但视图仍按同一处规则成帧：宽高比来自视口。
            view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
            light,
            drawables,
        })
    }

    /// 渲染 3D 场景视图（编辑场景 + 编辑器相机）到离屏纹理并更新 egui 预览。
    /// 内容未变化（修订/实际渲染尺寸/相机未动且无活动拖拽预览）时跳过渲染，沿用缓存纹理。
    ///
    /// M11-03：缓存 key 记录实际渲染尺寸（R-03，拖拽结束半分辨率 → 全分辨率必然失效）；
    /// 缓存只在回读成功后提交，失败清缓存并显示错误（R-04，下帧重试而非沿用旧帧）。
    pub(crate) fn render_scene_view_3d(&mut self, width: u32, height: u32, ctx: &egui::Context) {
        if width == 0 || height == 0 {
            return;
        }
        let revision =
            panel_scene_document(&self.play_session, &self.document).map(|doc| doc.revision);
        // Gizmo/拖拽预览变化需每帧重渲染；否则仅在修订/尺寸/相机变化时重渲染。
        let preview_active = !self.scene_state.preview3d_transform.is_empty();
        // 交互期（拖拽预览活动）降半分辨率渲染，空闲恢复全分辨率。
        let (render_w, render_h) = if preview_active {
            ((width / 2).max(1), (height / 2).max(1))
        } else {
            (width, height)
        };
        let cache = self.scene_view_cache_3d;
        // Play 期间也每帧重渲染：镜像里的姿态逐帧在变，而修订号只在结构变化时 bump。
        if !self.is_playing()
            && scene_view_3d_cache_hit(
                cache,
                render_w,
                render_h,
                revision,
                self.editor_camera,
                preview_active,
            )
        {
            self.drain_readback_3d(ctx);
            return;
        }
        let started = std::time::Instant::now();
        self.ensure_scene_renderer(render_w, render_h);
        // 前置条件未满足（无 GPU）时不更新缓存，下帧重试。
        if self.scene_renderer.is_none() || self.scene_texture.is_none() {
            return;
        }
        // M11-04 / R-05：确保编辑场景的 ECS 运行时已按当前文档 revision 建立，
        // 让 3D 视图与运行时共用同一条提取路径（相机以编辑器视图作为覆盖）。
        self.ensure_scene_runtime();
        let scene3d = self.build_scene3d(render_viewport(render_w, render_h));
        let mesh = SceneMesh {
            vertices: Vec::new(),
            draws: Vec::new(),
            world_sprites: Vec::new(),
            world_sprite_draws: Vec::new(),
            paths: Vec::new(),
            // 编辑器这条路径自己手写空网格：没有资源身份（渲染器回退到按路径解析）。
            path_handles: Vec::new(),
            font_atlas: None,
            custom_font_atlases: Vec::new(),
            vector_vertices: Vec::new(),
            vector_glyphs: Vec::new(),
            vector_segments: Vec::new(),
        };
        let material_preview_enabled = !self.is_playing();
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        let Some((texture, view)) = &self.scene_texture else {
            return;
        };
        // P3-E02：材质段的内存预览——不写盘、不改文档，只替换该路径本次渲染的解析结果。
        // Play 期间清掉预览，避免编辑器会话漏进运行时画面。
        if material_preview_enabled {
            renderer.set_material_preview(self.material.preview());
        } else {
            renderer.set_material_preview(None);
        }
        // WP5：编辑器视图也走统一的视图描述——3D 视图声明主视图（清屏色 + 视口），
        // 而不是让渲染器自己写死清屏色。
        // 拆分批 7：声明的主视图就是**提取时烘焙的那一份**（同一个视口烘焙一次）。
        if let Some(scene3d) = scene3d.as_ref() {
            renderer.set_frame_views(FrameViews {
                main: Some(scene3d.view),
                overlay: None,
            });
        }
        renderer.render_into_texture(&mesh, scene3d.as_ref(), view, render_w, render_h);
        if self.render_state.is_some() {
            // 原生路径（有窗口）：纹理已在 GPU 上，直接注册给 egui 采样——不回读、
            // 不重传，画面也不再滞后一帧。`scene_texture` 由 2D/3D 共用且刚被本视图
            // 覆盖，另一视图的缓存必须失效，否则切回去会命中缓存显示本视图的画面。
            self.sync_scene_view_texture();
            self.scene_view_cache_3d = SceneViewCache3d {
                size: (render_w, render_h),
                revision,
                camera: self.editor_camera,
            };
            self.scene_view_cache_2d = SceneViewCache2d::default();
        } else {
            // 无窗口（测试/无 GPU）：沿用非阻塞回读。
            // 就绪则上传并提交缓存；Pending 请求重绘等下帧轮询；
            // 失败清缓存（保持不匹配）让下帧重试，并显示用户可见错误（R-04）。
            match renderer.readback_frame(texture, render_w, render_h) {
                ReadbackStatus::Ready {
                    pixels,
                    width,
                    height,
                } => {
                    self.scene_view_cache_3d = SceneViewCache3d {
                        size: (render_w, render_h),
                        revision,
                        camera: self.editor_camera,
                    };
                    self.set_scene_3d_texture(pixels, width, height, ctx);
                }
                ReadbackStatus::Pending => ctx.request_repaint(),
                ReadbackStatus::Failed(error) => {
                    tracing::warn!(target: "editor", "[scene view] 3D readback failed: {error}");
                    self.scene_view_cache_3d = SceneViewCache3d::default();
                    self.set_scene_view_diagnostic(format!("3D 场景视图回读失败：{error}"));
                    ctx.request_repaint();
                }
            }
        }
        log_scene_view_frame(started, (render_w, render_h), "3D");
    }

    /// 确保编辑场景的 2D 运行时存在，且反映当前文档 revision 的场景内容。
    /// 场景编辑（revision 变化）时重建运行时，让背景随编辑更新。
    ///
    /// `pub(crate)`：拖动预览的提交层（`crate::scene_drag`）在推进第一帧预览前也要它。
    pub(crate) fn ensure_scene_runtime(&mut self) {
        let revision =
            panel_scene_document(&self.play_session, &self.document).map(|doc| doc.revision);
        // 已按此 revision 尝试过（无论成功失败）则不再重建，避免持久失败时每帧重试。
        if self.scene_runtime_revision == revision {
            return;
        }
        self.scene_runtime = None;
        let Some(scene) = panel_scene_document(&self.play_session, &self.document) else {
            self.scene_runtime_revision = revision;
            return;
        };
        let root = strip_verbatim_prefix(&self.project.root);
        // 共享字体图集：运行时随 revision 重建时复用同一实例，MSDF 字形只生成一次
        // （否则拖动 UI/精灵每帧重建运行时都会重新生成所有字形 → 卡顿）。
        if self.scene_font_atlas.is_none() {
            self.scene_font_atlas = load_project_font_atlas(Some(&root));
        }
        // 共享默认矢量图集：拖动/编辑导致运行时重建时，不重新解析大字体文件。
        if self.scene_vector_atlas.is_none() {
            self.scene_vector_atlas = load_project_vector_atlas(Some(&root));
        }
        match RuntimeScene::from_scene_edit(Some(&root), scene.scene.clone()) {
            Ok(mut runtime) => {
                // 捕获游戏 2D 相机中心（此时相机仍是运行时默认，未被子编辑器覆盖），
                // 供设计分辨率包围框定位游戏取景范围。
                self.game_camera_2d_center = runtime.camera_center_2d();
                // 场景视图渲染实际 UI（Text/Image/Button）覆盖层（默认 render_ui_overlay
                // = true），与包围框同用设计分辨率作 Canvas 参考。
                runtime.set_ui_reference([
                    self.preview_resolution.0 as f32,
                    self.preview_resolution.1 as f32,
                ]);
                runtime.set_font_atlas(self.scene_font_atlas.clone());
                runtime.set_custom_font_atlases(self.custom_font_atlases.clone());
                runtime.set_vector_atlas(self.scene_vector_atlas.clone());
                runtime.set_custom_vector_atlases(self.custom_vector_atlases.clone());
                self.scene_runtime = Some(runtime);
            }
            Err(error) => {
                tracing::warn!(target: "editor", "[scene view] 2D runtime rebuild failed: {error}");
            }
        }
        self.apply_scene_environment_settings();
        self.sync_particle_compute();
        self.scene_runtime_revision = revision;
    }

    /// P3-R07-1：把渲染器的 compute 能力同步给 Scene View 运行时。
    ///
    /// Scene View 与 Play 必须读同一份能力：两条路径判出不同落点，作者看到的就是
    /// "编辑器里说 `no_compute`、Play 里却说 `not_wired`"这类无法解释的差别。运行时或
    /// 渲染器缺席时不调用——那种形态下场景发射器本来就不跑。
    pub(crate) fn sync_particle_compute(&mut self) {
        let (Some(renderer), Some(runtime)) =
            (self.scene_renderer.as_ref(), self.scene_runtime.as_mut())
        else {
            return;
        };
        runtime.set_particle_compute_available(renderer.compute_enabled());
    }

    /// P3-R03-4：把编辑场景声明的环境应用到 Scene View 渲染器。
    ///
    /// 只在运行时按 revision 重建或渲染器首次创建时调用（`ensure_scene_*` 的早退已保证），
    /// 因此 cubemap 不会每帧重传。失败/回退原因写进 Scene View 诊断与日志，而不是静默降级。
    pub(crate) fn apply_scene_environment_settings(&mut self) {
        let Some(report) = self
            .scene_runtime
            .as_ref()
            .and_then(RuntimeScene::environment_report)
        else {
            if let Some(renderer) = self.scene_renderer.as_mut() {
                renderer.clear_environment();
            }
            return;
        };
        let (Some(renderer), Some(runtime)) =
            (self.scene_renderer.as_mut(), self.scene_runtime.as_ref())
        else {
            return;
        };
        let applied = apply_scene_environment(renderer, runtime, &report);
        if let Some(reason) = &applied.fallback_reason {
            tracing::warn!(target: "editor", "[scene view] {reason}");
        } else {
            tracing::info!(
                target: "editor",
                map = %report.resolved_path,
                face_size = report.face_size,
                mip_count = report.mip_count,
                ibl = applied.ibl_mode.as_str(),
                tier = applied.tier.as_str(),
                "[scene view] environment applied"
            );
        }
        if let Some(reason) = applied.fallback_reason {
            self.set_scene_view_diagnostic(format!("环境回退：{reason}"));
        }
    }

    /// 渲染 2D 场景视图（编辑场景，匹配编辑器 pan/zoom）到离屏纹理并更新 egui 预览。
    /// 内容未变化（修订/尺寸/相机未动）时跳过渲染，沿用缓存纹理。
    pub(crate) fn render_scene_view_2d(&mut self, width: u32, height: u32, ctx: &egui::Context) {
        if width == 0 || height == 0 {
            return;
        }
        let revision =
            panel_scene_document(&self.play_session, &self.document).map(|doc| doc.revision);
        let camera = (self.scene_state.pan, self.scene_zoom);
        let cache = self.scene_view_cache_2d;
        // Play 期间每帧重画：镜像里的姿态逐帧在变，而修订号只在结构变化时才 bump，
        // 只拿它当"要不要重画"的依据会让 Scene 视图停在进 Play 那一刻。
        let previewing = self.is_playing()
            || !self.scene_state.preview.is_empty()
            || !self.scene_state.preview_ui.is_empty();
        if !previewing
            && cache.size == (width, height)
            && cache.revision == revision
            && cache.camera == camera
        {
            self.drain_readback_2d(ctx);
            return;
        }
        // 前置条件未满足（无场景/无 GPU）时不更新缓存，下帧重试，避免"打开场景后
        // revision 仍为 0 被缓存命中"导致背景永不渲染。
        self.ensure_scene_runtime();
        if self.scene_runtime.is_none() {
            return;
        }
        self.ensure_scene_renderer(width, height);
        if self.scene_renderer.is_none() || self.scene_texture.is_none() {
            return;
        }
        // 本次实际渲染尺寸（2D 无半分辨率，即视口尺寸）；readback 返回的尺寸可能因
        // 滞后帧不同，缓存记录用实际渲染尺寸。
        let render_size = (width, height);
        let started = std::time::Instant::now();

        let zoom = self.scene_zoom;
        let pan = self.scene_state.pan;
        // 2D 相机匹配编辑器视图：屏幕中心对应世界 -pan/zoom。
        let camera2d =
            Camera2D::new(Vec2::new(-pan.x / zoom, -pan.y / zoom), zoom).unwrap_or_else(|_| {
                Camera2D::new(Vec2::new(0.0, 0.0), 1.0).expect("zero camera is valid")
            });
        let mesh = {
            let Some(runtime) = &mut self.scene_runtime else {
                return;
            };
            runtime.set_camera_2d(camera2d);
            // 编辑器视图只渲染世界内容；实际 UI（Text/Image/Button）由游戏画面
            // （设计分辨率 + 游戏相机）渲染进设计分辨率包围框内。
            runtime.set_render_ui_overlay(false);
            let mesh = match runtime.vertices(width, height) {
                Ok(mesh) => mesh,
                Err(error) => {
                    tracing::error!(target: "editor", "[scene view] 2D render failed: {error}");
                    self.set_scene_view_diagnostic(format!("场景视图渲染失败：{error}"));
                    return;
                }
            };
            self.clear_scene_view_diagnostic();
            mesh
        };
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        let Some((texture, view)) = &self.scene_texture else {
            return;
        };
        // WP5：2D 视图是这一帧的底 ⇒ 声明为主 2D 视图（清屏 + 自己的 pan/zoom 相机），
        // 与运行时用的是同一套 `RenderView` 说法。
        renderer.set_frame_views(FrameViews {
            main: Some(RenderView::main_2d(
                camera2d,
                render_viewport(width, height),
                DEFAULT_3D_CLEAR,
            )),
            overlay: None,
        });
        renderer.render_into_texture(&mesh, None, view, width, height);
        if self.render_state.is_some() {
            // 原生路径（有窗口）：纹理已在 GPU 上，直接注册给 egui 采样——不回读、
            // 不重传，画面也不再滞后一帧。`scene_texture` 由 2D/3D 共用且刚被本视图
            // 覆盖，另一视图的缓存必须失效，否则切回去会命中缓存显示本视图的画面。
            self.sync_scene_view_texture();
            self.scene_view_cache_2d = SceneViewCache2d {
                size: render_size,
                revision,
                camera,
            };
            self.scene_view_cache_3d = SceneViewCache3d::default();
        } else {
            // 无窗口（测试/无 GPU）：沿用非阻塞回读。
            // 就绪则上传并提交缓存；Pending 请求重绘等下帧轮询；
            // 失败清缓存（保持不匹配）让下帧重试，并显示用户可见错误（R-04）。
            match renderer.readback_frame(texture, render_size.0, render_size.1) {
                ReadbackStatus::Ready {
                    pixels,
                    width,
                    height,
                } => {
                    self.scene_view_cache_2d = SceneViewCache2d {
                        size: render_size,
                        revision,
                        camera,
                    };
                    self.set_scene_2d_texture(pixels, width, height, ctx);
                }
                ReadbackStatus::Pending => {
                    self.set_scene_view_diagnostic("场景视图等待 GPU 回读……显示上一帧");
                    ctx.request_repaint();
                }
                ReadbackStatus::Failed(error) => {
                    tracing::warn!(target: "editor", "[scene view] 2D readback failed: {error}");
                    self.scene_view_cache_2d = SceneViewCache2d::default();
                    self.set_scene_view_diagnostic(format!("场景视图回读失败：{error}"));
                    ctx.request_repaint();
                }
            }
        }
        // 渲染游戏画面（设计分辨率 + 游戏相机 + UI）到设计分辨率包围框内。
        // 两条路径都要画：原生路径只是省掉了编辑器视图本身的回读。
        self.render_scene_screen(ctx);
        log_scene_view_frame(started, render_size, "2D");
    }

    /// 设计分辨率包围框里那格"游戏画面"该显示哪张纹理。
    ///
    /// Play 期间它是 **Game 视图正在显示的那一帧**（`PreviewSession` 的预览纹理）：镜像文档
    /// 只带结构 + 姿态（`runtime_sync::upsert_spawned_rows` 新建的行除 `Transform` 与名字外
    /// 没有 `Image`/`Text`/`UiLayout` 载荷——见 docs/56 §3.53 的诚实边界），拿它重画一遍只会
    /// 得到一块空白；而 Game 视图那一帧本来就是"游戏此刻的真值"，两者同源才叫同步。
    /// 编辑期间沿用编辑运行时渲染出来的 `scene_screen_tex_id`。
    pub(crate) fn game_screen_texture_id(&self) -> Option<egui::TextureId> {
        game_screen_texture(
            self.is_playing(),
            self.preview.as_ref().and_then(PreviewSession::texture_id),
            self.scene_screen_tex_id,
        )
    }

    /// 渲染游戏画面（设计分辨率 + 游戏相机 + UI 覆盖层）到离屏纹理，并注册为原生
    /// egui 纹理，供 Scene 视图在设计分辨率包围框内显示。内容未变化（修订/设计
    /// 分辨率未动）时跳过渲染；失败时保留上一帧（诊断由编辑器视图负责）。
    pub(crate) fn render_scene_screen(&mut self, ctx: &egui::Context) {
        // Play 期间这一格走的是 [`Self::game_screen_texture_id`] 那张预览纹理，**不再**用
        // 无脚本的镜像运行时重画一遍：那既是错的数据（见上），也是白付的渲染成本
        // （docs/57：Debug 下每帧 90~240ms，直接把预览的固定步挤掉）。缓存键清掉是为了
        // Stop 之后第一帧必定重画编辑运行时那份——否则会命中 Play 前那次的老纹理。
        if self.is_playing() {
            self.scene_screen_cache_key = None;
            return;
        }
        let revision =
            panel_scene_document(&self.play_session, &self.document).map(|doc| doc.revision);
        let (dw, dh) = self.preview_resolution;
        if dw == 0 || dh == 0 {
            return;
        }
        let key = revision.map(|r| (r, self.preview_resolution));
        // Play 期间每帧重画：镜像里的姿态逐帧在变，而修订号只在结构变化时才 bump，
        // 只拿它当"要不要重画"的依据会让 Scene 视图停在进 Play 那一刻。
        let previewing = self.is_playing()
            || !self.scene_state.preview.is_empty()
            || !self.scene_state.preview_ui.is_empty();
        if !previewing && self.scene_screen_cache_key == key && self.scene_screen_tex_id.is_some() {
            return;
        }
        if self.scene_renderer.is_none() || self.scene_runtime.is_none() {
            return;
        }
        self.ensure_scene_screen_texture(dw, dh);
        if self.scene_screen_texture.is_none() {
            return;
        }
        // 游戏相机 + UI 覆盖层：渲染游戏画面（与预览一致，相机中心 = 默认 2D 相机）。
        let game_camera = Camera2D::new(
            Vec2::new(self.game_camera_2d_center.x, self.game_camera_2d_center.y),
            1.0,
        )
        .expect("default camera center is valid");
        let mesh = {
            let runtime = self.scene_runtime.as_mut().expect("scene runtime checked");
            runtime.set_camera_2d(game_camera);
            runtime.set_render_ui_overlay(true);
            // 运行时只在场景 revision 变化时重建；切换设计分辨率时需同步 Canvas
            // 参考分辨率，否则渲染的 UI 用旧 reference 缩放（viewport/reference≠1），
            // 与包围框（当前设计分辨率、scale=1）错位。这里每帧重渲染前刷新。
            runtime.set_ui_reference([dw as f32, dh as f32]);
            match runtime.vertices(dw, dh) {
                Ok(mesh) => mesh,
                Err(error) => {
                    tracing::warn!(
                        target: "editor",
                        "[scene view] game screen render failed: {error}"
                    );
                    return;
                }
            }
        };
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        let Some((_, view)) = &self.scene_screen_texture else {
            return;
        };
        // 3D 也要进游戏画面：与 Play 的 `PreviewSession` 同一条提取。`vertices()` 刚在
        // 同视口定过帧，`scene_3d` 直接吃那一份（纯 2D 场景返回 None）。
        let viewport = render_viewport(dw, dh);
        let scene3d = self
            .scene_runtime
            .as_ref()
            .and_then(|runtime| runtime.scene_3d(viewport));
        renderer.set_frame_views(game_screen_frame_views(
            game_camera,
            viewport,
            scene3d.is_some(),
        ));
        renderer.render_into_texture(&mesh, scene3d.as_ref(), view, dw, dh);
        self.sync_scene_screen_texture();
        self.scene_screen_cache_key = key;
        ctx.request_repaint();
    }

    /// 确保游戏画面离屏纹理存在且尺寸与设计分辨率一致。
    pub(crate) fn ensure_scene_screen_texture(&mut self, width: u32, height: u32) {
        let size_changed = self
            .scene_screen_texture
            .as_ref()
            .is_none_or(|(t, _)| t.width() != width || t.height() != height);
        if !size_changed {
            return;
        }
        let Some((device, _)) = &self.gpu else {
            return;
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-scene-screen"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.scene_screen_texture = Some((texture, view));
        self.scene_screen_cache_key = None;
    }

    /// 把游戏画面纹理注册/更新为原生 egui 纹理（免 CPU 回读，避免与编辑器视图回读混淆）。
    pub(crate) fn sync_scene_screen_texture(&mut self) {
        let Some(render_state) = &self.render_state else {
            return;
        };
        let Some((device, _)) = &self.gpu else {
            return;
        };
        let Some((_, view)) = &self.scene_screen_texture else {
            return;
        };
        let mut renderer = render_state.renderer.write();
        match self.scene_screen_tex_id {
            Some(id) => {
                renderer.update_egui_texture_from_wgpu_texture(
                    device,
                    view,
                    wgpu::FilterMode::Nearest,
                    id,
                );
            }
            None => {
                let id = renderer.register_native_texture(device, view, wgpu::FilterMode::Nearest);
                self.scene_screen_tex_id = Some(id);
            }
        }
    }

    /// 把编辑器视图（2D/3D）的离屏纹理注册/更新为原生 egui 纹理。
    ///
    /// 与 `sync_scene_screen_texture` 同一套做法：保持各自注册时的过滤模式
    /// （2D 像素风用 Nearest，3D 用 Linear）。没有窗口时什么都不做——原生 ID 只在
    /// `render_state` 存在时才可能被创建，此时调用方本就走回读回退。
    pub(crate) fn sync_scene_view_texture(&mut self) {
        let Some(render_state) = &self.render_state else {
            return;
        };
        let Some((device, _)) = &self.gpu else {
            return;
        };
        let Some((_, view)) = &self.scene_texture else {
            return;
        };
        let mut renderer = render_state.renderer.write();
        sync_native_scene_texture(
            &mut renderer,
            &mut self.scene_2d_tex,
            device,
            view,
            wgpu::FilterMode::Nearest,
        );
        sync_native_scene_texture(
            &mut renderer,
            &mut self.scene_preview_tex,
            device,
            view,
            wgpu::FilterMode::Linear,
        );
    }

    /// 释放 2D/3D 视图的 egui 原生纹理 ID。
    ///
    /// 原生 ID 只是指向 `scene_texture` 绑定组的句柄：离屏纹理被重建或销毁后，
    /// 旧 ID 会一直指向已废弃的纹理（采样不到内容），所以必须先释放再重新注册。
    /// `pub(crate)`：关窗时 shell 的 `on_exit` 也要用它归还 ID。
    pub(crate) fn release_scene_view_textures(&mut self) {
        let Some(render_state) = &self.render_state else {
            self.scene_2d_tex = None;
            self.scene_preview_tex = None;
            return;
        };
        let mut renderer = render_state.renderer.write();
        for slot in [&mut self.scene_2d_tex, &mut self.scene_preview_tex] {
            if let Some(SceneViewTexture::Native(id)) = slot {
                renderer.free_texture(id);
            }
            *slot = None;
        }
    }

    /// 把 2D 回读像素上传到 egui 纹理（NEAREST，像素风）。
    pub(crate) fn set_scene_2d_texture(
        &mut self,
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        ctx: &egui::Context,
    ) {
        let image =
            egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &pixels);
        match &mut self.scene_2d_tex {
            Some(SceneViewTexture::Uploaded(tex)) => tex.set(image, egui::TextureOptions::NEAREST),
            _ => {
                self.scene_2d_tex = Some(SceneViewTexture::Uploaded(ctx.load_texture(
                    "scene_2d",
                    image,
                    egui::TextureOptions::NEAREST,
                )));
            }
        }
    }

    /// 把 3D 回读像素上传到 egui 纹理（LINEAR 平滑缩放）。
    pub(crate) fn set_scene_3d_texture(
        &mut self,
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        ctx: &egui::Context,
    ) {
        let image =
            egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &pixels);
        match &mut self.scene_preview_tex {
            Some(SceneViewTexture::Uploaded(tex)) => tex.set(image, egui::TextureOptions::LINEAR),
            _ => {
                self.scene_preview_tex = Some(SceneViewTexture::Uploaded(ctx.load_texture(
                    "scene_view",
                    image,
                    egui::TextureOptions::LINEAR,
                )));
            }
        }
    }

    /// 内容未变化时排空 2D 在途回读（补上滞后一帧的纹理），并自终止重绘轮询。
    pub(crate) fn drain_readback_2d(&mut self, ctx: &egui::Context) {
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        if !renderer.readback_in_flight() {
            return;
        }
        match renderer.readback_pending() {
            ReadbackStatus::Ready {
                pixels,
                width,
                height,
            } => {
                self.set_scene_2d_texture(pixels, width, height, ctx);
            }
            ReadbackStatus::Pending => ctx.request_repaint(),
            ReadbackStatus::Failed(error) => {
                tracing::warn!(target: "editor", "[scene view] 2D readback drain failed: {error}");
            }
        }
    }

    /// 内容未变化时排空 3D 在途回读（补上滞后一帧的纹理），并自终止重绘轮询。
    pub(crate) fn drain_readback_3d(&mut self, ctx: &egui::Context) {
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        if !renderer.readback_in_flight() {
            return;
        }
        match renderer.readback_pending() {
            ReadbackStatus::Ready {
                pixels,
                width,
                height,
            } => {
                self.set_scene_3d_texture(pixels, width, height, ctx);
            }
            ReadbackStatus::Pending => ctx.request_repaint(),
            ReadbackStatus::Failed(error) => {
                tracing::warn!(target: "editor", "[scene view] 3D readback drain failed: {error}");
            }
        }
    }
}

/// 游戏画面的视图声明。
///
/// 纯 2D 时主视图就是游戏相机（清屏 + 设计分辨率视口）；**有 3D 时不能声明 2D 主视图**：
/// 渲染器把声明的 `RenderView` 优先当作 3D 的投影矩阵（`passes.rs` 的 `main_matrix`），
/// 3D 会被 2D 正交矩阵拍扁。有 3D 时与 Play 的 `PreviewSession` 一致——不声明，让渲染器
/// 回退到场景提取时按同一视口烘焙的 `Scene3d::view`。
pub(crate) fn game_screen_frame_views(
    camera: Camera2D,
    viewport: Viewport,
    has_3d: bool,
) -> FrameViews {
    if has_3d {
        return FrameViews {
            main: None,
            overlay: None,
        };
    }
    FrameViews {
        main: Some(RenderView::main_2d(camera, viewport, DEFAULT_3D_CLEAR)),
        overlay: None,
    }
}
