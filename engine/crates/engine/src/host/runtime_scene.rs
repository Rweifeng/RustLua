//! Runtime scene: instantiates the project scene/scripts and produces
//! per-frame [`SceneMesh`] (2D) and [`Scene3d`] draw data for the renderer.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::core::{
    animation::AnimationStateMachine,
    animation3d::Animation3D,
    camera3d::Camera3D,
    entity::EntityId,
    font::FontAtlas,
    input::InputSnapshot,
    light::Light,
    math2d::{Camera2D, Viewport},
    math3d::Vec3,
    mesh3d::MeshRenderer,
    scene::{Scene, SceneEntityId},
    script::LuaRuntime,
    sprite::{Sprite, SpriteAsset, SpriteSorting, SpriteVertex},
    sprite_batch::{SpriteBatch, SpriteBatchItem, SpriteBatchMesh, SpriteDrawBatch},
    text::{TextAtlas, TextLayout, TextMesh, TextStyle},
    tilemap::Tilemap,
    transform::{Transform, Vec2, integrate_velocity},
    transform3d::Transform3D,
    ui::{
        UI_REFERENCE_SIZE, UiImage, UiText, nine_slice_cells, resolve_ui_rect,
        resolve_ui_text_origin, ui_text_effective_scale,
    },
};

use crate::host::frame::{
    ATLAS_TEXTURE_PATH, BoundsSource, FONT_ATLAS_TEXTURE_PATH, FontAtlasPixels, OverlayVertex,
    Scene3d, Scene3dBounds, Scene3dDrawable, SceneDraw, SceneMesh, WHITE_TEXTURE_PATH,
};

pub struct RuntimeScene {
    script: LuaRuntime,
    sprite_batch: SpriteBatch,
    camera: Camera2D,
    viewport_width: u32,
    viewport_height: u32,
    /// Canvas 空间 UI 参考分辨率（设计分辨率）。发布游戏默认 `UI_REFERENCE_SIZE`；
    /// 编辑器按设计分辨率显式设置，使 UI 适配 = viewport / reference。
    ui_reference: [f32; 2],
    /// 是否渲染屏幕空间 UI 覆盖层。编辑器场景视图置 false：UI 由 egui 在
    /// 设计分辨率包围框内绘制布局框，避免旧的面板适配覆盖层与之冲突。
    render_ui_overlay: bool,
    /// 项目根目录（纹理路径解析用，与渲染器 `project_root.join(path)` 一致）。
    project_root: Option<PathBuf>,
    /// 九宫格 UV 计算用的纹理像素尺寸缓存（惰性、按 path）。
    texture_dims: HashMap<String, (u32, u32)>,
    texture_paths: Vec<String>,
    texture_lookup: HashMap<String, u32>,
    tilemap_cache: HashMap<EntityId, CachedTilemapLayer>,
    /// 共享字体图集：运行时重建（编辑器拖动/编辑场景导致 revision 变化时）复用
    /// 同一实例，避免每帧重新生成 MSDF 字形。生成字形需可变访问（内部 Mutex）。
    font_atlas: Option<Arc<Mutex<FontAtlas>>>,
    last_font_version: u64,
}

#[derive(Clone, Copy)]
struct CachedTileSprite {
    center_offset: Vec2,
    uv_min: Vec2,
    uv_max: Vec2,
}

struct CachedTilemapLayer {
    transform: Transform,
    tilemap: Tilemap,
    sprites: Vec<CachedTileSprite>,
}

impl RuntimeScene {
    pub fn new(project_root: Option<&Path>) -> Result<Self, String> {
        let scene = load_runtime_scene(project_root);
        Self::from_scene(project_root, scene)
    }

    /// Instantiates a specific scene (used by the editor to run the scene being
    /// edited), rather than the project's startup scene.
    pub fn from_scene(project_root: Option<&Path>, scene: Scene) -> Result<Self, String> {
        Self::init(project_root, scene, true)
    }

    /// Instantiates a scene WITHOUT running the project script, for the editor's
    /// scene view (render the scene as edited, no game logic).
    pub fn from_scene_edit(project_root: Option<&Path>, scene: Scene) -> Result<Self, String> {
        Self::init(project_root, scene, false)
    }

    /// 设置 2D 相机（编辑器场景视图用，匹配编辑器 pan/zoom）。
    pub fn set_camera_2d(&mut self, camera: Camera2D) {
        self.camera = camera;
    }

    /// 当前 2D 相机中心（世界坐标）。编辑器 Scene 视图用它定位设计分辨率包围框，
    /// 使包围框与游戏取景范围一致（默认 `DEFAULT_2D_CAMERA_CENTER`）。
    pub fn camera_center_2d(&self) -> Vec2 {
        self.camera.center
    }

    /// 设置 Canvas 空间 UI 的参考分辨率（设计分辨率）。同步给 Lua 侧，
    /// 保证渲染与按钮命中测试（`process_ui_clicks`）用同一参考。
    pub fn set_ui_reference(&mut self, reference: [f32; 2]) {
        self.ui_reference = reference;
        self.script.set_ui_reference(reference);
    }

    /// 是否渲染屏幕空间 UI 覆盖层。编辑器场景视图置 false（UI 由 egui 在
    /// 设计分辨率包围框内绘制）；游戏预览保持 true。
    pub fn set_render_ui_overlay(&mut self, enabled: bool) {
        self.render_ui_overlay = enabled;
    }

    /// 注入共享字体图集（编辑器在运行时重建时复用，避免每帧重新生成 MSDF 字形）。
    pub fn set_font_atlas(&mut self, font_atlas: Option<Arc<Mutex<FontAtlas>>>) {
        self.font_atlas = font_atlas;
    }

    /// 惰性加载项目字体图集：未注入共享图集时才按需加载（含 chars.txt 预热）。
    /// 编辑器场景视图/预览会先 `set_font_atlas` 注入已预热图集，故这里通常不会触发；
    /// 独立游戏/测试首次排版文本时加载一次，此后复用。
    fn ensure_font_atlas(&mut self) {
        if self.font_atlas.is_none()
            && let Some(root) = &self.project_root
        {
            self.font_atlas = load_project_font_atlas(Some(root));
        }
    }

    /// 度量一段场景文本在给定 scale 下的网格尺寸与字形像素范围
    /// `(size, pixel_bounds)`（编辑器 UI 包围框用）。`size` 含
    /// ascender/descender 空白（锚点对齐用），`pixel_bounds` 是可见字形的
    /// 紧致范围（包围框贴合文字用）。与渲染同路径：SDF 字体存在时用真实
    /// 字体，否则用内置 5x7 位图。
    pub fn measure_ui_text(&mut self, text: &str, scale: f32) -> ([f32; 2], [f32; 4]) {
        self.ensure_font_atlas();
        let style = TextStyle {
            scale,
            ..Default::default()
        };
        if let Some(font) = &self.font_atlas {
            let mut font = font.lock().expect("font atlas lock");
            TextLayout::build(text, [0.0, 0.0], style, &mut font)
                .map(|mesh| (mesh.size, mesh.pixel_bounds()))
                .unwrap_or(([0.0, 0.0], [0.0; 4]))
        } else {
            let atlas = TextAtlas::built_in();
            TextLayout::build_bitmap(text, [0.0, 0.0], style, &atlas)
                .map(|mesh| (mesh.size, mesh.pixel_bounds()))
                .unwrap_or(([0.0, 0.0], [0.0; 4]))
        }
    }

    fn init(project_root: Option<&Path>, scene: Scene, run_script: bool) -> Result<Self, String> {
        let mut script = LuaRuntime::new().map_err(|error| error.to_string())?;
        // Project root must be set before instantiation so scene components that
        // load assets (e.g. Animation3D) can resolve their mesh paths.
        if let Some(root) = project_root {
            script.set_project_root(root.to_path_buf());
        }
        let instantiated = if run_script {
            script
                .instantiate_scene(&scene)
                .map_err(|error| error.to_string())?
        } else {
            script
                .instantiate_scene_with_editor_visibility(&scene, true)
                .map_err(|error| error.to_string())?
        };
        tracing::info!(target: "scene", "[scene] loaded name={} entities={}", scene.name, scene.entities.len());
        tracing::info!(target: "ecs", "[ecs] world initialized entities={instantiated}");
        if run_script && let Some(root) = project_root {
            script.set_audio_asset_dir(root.join("assets").join("audio"));
            script.set_save_path(root.join(crate::core::PROJECT_WORK_DIR).join("save.sav"));
            let script_path = root.join("scripts/main.lua");
            if script_path.is_file() {
                script
                    .execute_project_script(&script_path)
                    .map_err(|error| error.to_string())?;
            } else {
                tracing::info!(target: "script", path = %script_path.display(), "[script] game project has no scripts/main.lua; script runtime idle");
            }
        }
        let camera = Camera2D::new(crate::core::math2d::DEFAULT_2D_CAMERA_CENTER, 1.0)
            .map_err(|error| error.to_string())?;
        tracing::info!(target: "render", "[render] SpriteBatch and Camera2D initialized");
        // 字体图集不再在此无条件加载：`load_project_font_atlas` 会按 chars.txt 预热
        // 全部 MSDF 字形，debug 构建下约 2s/项目（实测 ui_demo 135 字 ≈ 2.3s）。
        // 编辑器场景视图/预览通过 `set_font_atlas` 注入共享（已预热）图集；未注入
        // 时由 `ensure_font_atlas` 在首次排版文本时按需加载。这样场景编辑（revision
        // 变化 → 运行时重建）不再每次重建都白白预热一遍被丢弃的图集。
        Ok(Self {
            script,
            sprite_batch: SpriteBatch::new(),
            camera,
            viewport_width: 1280,
            viewport_height: 720,
            ui_reference: UI_REFERENCE_SIZE,
            render_ui_overlay: true,
            project_root: project_root.map(|root| root.to_path_buf()),
            texture_dims: HashMap::new(),
            texture_paths: Vec::new(),
            texture_lookup: HashMap::new(),
            tilemap_cache: HashMap::new(),
            font_atlas: None,
            last_font_version: u64::MAX,
        })
    }

    pub fn run_script_steps(
        &mut self,
        steps: u32,
        delta_seconds: f32,
        input: &InputSnapshot,
    ) -> Result<(), String> {
        self.script.set_delta(delta_seconds);
        let mut step_input = input.clone();
        for step in 0..steps {
            self.script.set_input(step_input.clone());
            let result = self
                .script
                .call_update()
                .map_err(|error| error.to_string())?;
            tracing::debug!(target: "game", "[simulation] fixed update returned: {result}");
            // clickEvent 点击分发：预计算命中矩形（Image/Text UI 矩形 + Sprite 相机
            // 投影）→ 按下边沿命中 → Lua on_click(name)。
            let targets = self.compute_click_targets();
            self.script
                .process_ui_clicks(&step_input, &targets)
                .map_err(|error| error.to_string())?;
            // Physics advances on the fixed timestep and writes dynamic body
            // positions back into ECS Transforms (2D) / Transform3D (3D).
            self.script.step_physics(delta_seconds);
            self.script.step_physics3d(delta_seconds);
            self.script.step_particles(delta_seconds);
            self.script.step_clock(delta_seconds);
            // Apply a pending scene.load so level switches happen between steps.
            self.script
                .process_scene_transition()
                .map_err(|error| error.to_string())?;
            // One-shot edges are consumed by the first fixed step of a frame;
            // clear them so catch-up steps in the same frame don't re-fire
            // just_pressed actions (e.g. F4 switching scenes twice).
            if step == 0 {
                step_input.finish_frame();
            }
        }
        self.script.with_world_mut(|world| {
            let _ = integrate_velocity(world, delta_seconds * steps as f32);
            let elapsed = delta_seconds * steps as f32;
            world.for_each_mut::<AnimationStateMachine>(|_, machine| machine.advance(elapsed));
            world.for_each_mut::<Animation3D>(|_, animation| animation.advance(elapsed));
        });
        Ok(())
    }

    /// 计算当前所有 clickEvent 实体的命中矩形 `(实体名, origin, size)`（屏幕像素）。
    ///
    /// Image/Text 用 UI 解析（viewport=实际渲染尺寸、reference=设计分辨率），
    /// Sprite 用相机投影到屏幕；供 `process_ui_clicks` 点选分发。
    fn compute_click_targets(&mut self) -> Vec<(String, [f32; 2], [f32; 2])> {
        let viewport = Viewport {
            width: self.viewport_width.max(1) as f32,
            height: self.viewport_height.max(1) as f32,
        };
        let viewport_arr = [viewport.width, viewport.height];
        let reference = self.ui_reference;
        // 先提取 clickEvent 实体及其视觉组件（world 借用结束后再计算，避免借用冲突）。
        let entities: Vec<(
            EntityId,
            Option<UiImage>,
            Option<UiText>,
            Option<(Sprite, Transform)>,
        )> = self.script.with_world(|world| {
            let mut out = Vec::new();
            for id in world.entities_with::<crate::core::ui::ClickEvent>() {
                let image = world.get::<UiImage>(id).ok().flatten().cloned();
                let text = world.get::<UiText>(id).ok().flatten().cloned();
                let sprite = world.get::<Sprite>(id).ok().flatten().cloned();
                let transform = world.get::<Transform>(id).ok().flatten().cloned();
                out.push((id, image, text, sprite.zip(transform)));
            }
            out
        });
        let camera = self.camera;
        entities
            .into_iter()
            .filter_map(|(id, image, text, sprite)| {
                let name = self.script.entity_name(id)?;
                let rect = if let Some(image) = image {
                    resolve_ui_rect(
                        image.position,
                        image.size,
                        image.space,
                        image.anchor,
                        viewport_arr,
                        reference,
                    )
                } else if let Some(text) = text {
                    self.text_hit_rect(&text, viewport_arr, reference)?
                } else if let Some((sprite, transform)) = sprite {
                    sprite_screen_rect(&sprite, &transform, camera, viewport)?
                } else {
                    return None;
                };
                Some((name, rect.0, rect.1))
            })
            .collect()
    }

    /// 文本 hit 矩形：与编辑器 UI 包围框同款——resolve 出网格原点，度量字形
    /// bbox，返回 `(glyph_origin, glyph_size)`（屏幕像素）。
    fn text_hit_rect(
        &mut self,
        text: &UiText,
        viewport: [f32; 2],
        reference: [f32; 2],
    ) -> Option<([f32; 2], [f32; 2])> {
        let effective_scale = text.effective_scale();
        let (_, resolved_scale) = resolve_ui_text_origin(
            text.position,
            [0.0, 0.0],
            effective_scale,
            text.space,
            text.anchor,
            viewport,
            reference,
        );
        let (text_size, glyph_bounds) = self.measure_ui_text(&text.text, resolved_scale);
        let (origin, _) = resolve_ui_text_origin(
            text.position,
            text_size,
            effective_scale,
            text.space,
            text.anchor,
            viewport,
            reference,
        );
        let glyph_origin = [origin[0] + glyph_bounds[0], origin[1] + glyph_bounds[1]];
        let glyph_size = [
            (glyph_bounds[2] - glyph_bounds[0]).max(0.0),
            (glyph_bounds[3] - glyph_bounds[1]).max(0.0),
        ];
        Some((glyph_origin, glyph_size))
    }

    /// Gathers the per-frame 3D render state from the runtime world: the active
    /// camera (Lua-selected, else the first `Camera3D` entity, else a default),
    /// the active light, and every entity with both a `Transform3D` and a
    /// `MeshRenderer`. Returns `None` when the scene has no 3D drawables.
    pub fn scene_3d(&self, aspect_ratio: f32) -> Option<Scene3d> {
        self.scene_3d_with_camera(aspect_ratio, None)
    }

    /// 带可选相机覆盖的 3D 提取（M11-04 / R-05）：编辑器 Scene 3D 视图与运行时
    /// 共用同一条 ECS drawable/component 提取路径（单一事实来源），编辑器传入
    /// 视图导航相机作为覆盖（不改场景数据），运行时传入 `None` 走
    /// `active_camera3d`（Lua 选择 / 首个 Camera3D / 默认）。
    pub fn scene_3d_with_camera(
        &self,
        aspect_ratio: f32,
        camera_override: Option<Camera3D>,
    ) -> Option<Scene3d> {
        // Runtime extraction intentionally reads ECS state so physics, scripts,
        // active camera/light selection, and animation palettes remain live.
        let drawables = self.script.with_world(|world| {
            let mut drawables = Vec::new();
            world.for_each2::<Transform3D, MeshRenderer>(|id, transform, renderer| {
                let Ok(model) = transform.model_matrix() else {
                    return;
                };
                let name = self
                    .script
                    .entity_name(id)
                    .unwrap_or_else(|| format!("Entity {}", id.index()));
                let palette = world
                    .get::<Animation3D>(id)
                    .ok()
                    .flatten()
                    .map(|animation| animation.joint_matrices())
                    .unwrap_or_default();
                let bounds = if renderer.mesh == crate::core::mesh3d::GROUND_PLANE_SENTINEL {
                    Scene3dBounds {
                        local_aabb: crate::core::math3d::Aabb3::new(
                            Vec3::new(-100.0, -0.05, -100.0),
                            Vec3::new(100.0, 0.05, 100.0),
                        )
                        .unwrap_or_else(|_| crate::core::math3d::Aabb3 {
                            min: Vec3::new(-1.0, -1.0, -1.0),
                            max: Vec3::new(1.0, 1.0, 1.0),
                        }),
                        source: BoundsSource::Fallback,
                    }
                } else {
                    Scene3dBounds {
                        local_aabb: crate::core::math3d::Aabb3 {
                            min: Vec3::new(-0.5, -0.5, -0.5),
                            max: Vec3::new(0.5, 0.5, 0.5),
                        },
                        source: BoundsSource::Fallback,
                    }
                };
                drawables.push(Scene3dDrawable {
                    entity: SceneEntityId(id.index() as u64),
                    name,
                    mesh_path: renderer.mesh.clone(),
                    model,
                    palette,
                    bounds,
                });
            });
            drawables
        });
        if drawables.is_empty() {
            return None;
        }
        // 编辑器视图覆盖相机时不触碰场景数据（不写 ECS、不改脚本状态），仅作为
        // 渲染视角覆盖；运行时传 None 则按 Lua 选择 / 首个 Camera3D / 默认解析。
        let camera = camera_override
            .map(|mut c| {
                c.aspect_ratio = aspect_ratio;
                c
            })
            .unwrap_or_else(|| self.active_camera3d(aspect_ratio));
        let light = self.active_light();
        Some(Scene3d {
            camera,
            light,
            drawables,
        })
    }

    /// Resolves the active camera: Lua's `camera.set_active` selection, else the
    /// first entity with a `Camera3D` component, else a fixed default.
    fn active_camera3d(&self, aspect_ratio: f32) -> Camera3D {
        let fallback = Camera3D::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            aspect_ratio,
            0.1,
            100.0,
        )
        .unwrap_or_else(|_| Camera3D {
            position: Vec3::new(0.0, 0.0, 5.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: std::f32::consts::FRAC_PI_2,
            aspect_ratio,
            near: 0.1,
            far: 100.0,
        });
        let selected = self.script.active_camera();
        let mut camera = self.script.with_world(|world| {
            let mut selected_camera = None;
            let mut fallback = None;
            world.for_each::<Camera3D>(|id, camera| {
                if fallback.is_none() {
                    fallback = Some(*camera);
                }
                if selected == Some(id) {
                    selected_camera = Some(*camera);
                }
            });
            selected_camera.or(fallback)
        });
        if let Some(camera) = &mut camera {
            camera.aspect_ratio = aspect_ratio;
            return *camera;
        }
        fallback
    }

    /// Resolves the active light: Lua's `light.set_active` selection, else the
    /// first entity with a `Light` component, else a default directional light.
    fn active_light(&self) -> Light {
        let selected = self.script.active_light();
        self.script
            .with_world(|world| {
                let mut selected_light = None;
                let mut fallback = None;
                world.for_each::<Light>(|id, light| {
                    if fallback.is_none() {
                        fallback = Some(*light);
                    }
                    if selected == Some(id) {
                        selected_light = Some(*light);
                    }
                });
                selected_light.or(fallback)
            })
            .unwrap_or_default()
    }

    pub fn vertices(&mut self, width: u32, height: u32) -> Result<SceneMesh, String> {
        self.viewport_width = width.max(1);
        self.viewport_height = height.max(1);
        self.script.set_ui_viewport(width, height);
        let viewport = Viewport::new(width.max(1) as f32, height.max(1) as f32)
            .map_err(|error| error.to_string())?;
        self.sprite_batch.clear();
        self.texture_paths.clear();
        self.texture_lookup.clear();
        let sprites = self.script.with_world(|world| {
            let mut sprites = Vec::new();
            world.for_each::<Sprite>(|id, sprite| {
                let Ok(Some(transform)) = world.get::<Transform>(id) else {
                    return;
                };
                let animation = world
                    .get::<AnimationStateMachine>(id)
                    .ok()
                    .flatten()
                    .cloned();
                let asset = world
                    .get::<SpriteAsset>(id)
                    .ok()
                    .flatten()
                    .map(|entry| entry.path().to_owned());
                let sorting = world.get::<SpriteSorting>(id).ok().flatten().cloned();
                sprites.push((*transform, *sprite, animation, asset, sorting));
            });
            sprites
        });
        // Tile maps draw first (layer 0), sprite entities on top (layer 1+).
        let tilemap_mesh = self.build_tilemap_mesh(viewport)?;
        for (index, (transform, sprite, animation, asset, sorting)) in
            sprites.into_iter().enumerate()
        {
            let (texture, sprite) = if let Some(machine) = animation {
                // Animated sprite: sample the current clip's sheet at its frame UV.
                let (uv_min, uv_max) = machine.current_frame_uv();
                let sprite = sprite
                    .with_uv(uv_min, uv_max)
                    .map_err(|error| error.to_string())?;
                let sheet = machine.current_clip().sheet.clone();
                (self.texture_id(&sheet), sprite)
            } else if let Some(path) = asset {
                (self.texture_id(&path), sprite)
            } else {
                continue;
            };
            let layer = sorting
                .as_ref()
                .map(SpriteSorting::effective_layer)
                .unwrap_or(1 + index as i32);
            self.sprite_batch
                .push(SpriteBatchItem::new(texture, layer, sprite, transform));
        }
        self.push_particles()?;
        let mesh = self
            .sprite_batch
            .build(self.camera, viewport)
            .map_err(|error| error.to_string())?;
        let mesh = merge_sprite_meshes(tilemap_mesh, mesh);
        tracing::debug!(
            target: "render",
            "[render] batch items={} vertices={} draw_ranges={}",
            self.sprite_batch.len(),
            mesh.vertices.len(),
            mesh.draws.len()
        );
        let mut draws: Vec<SceneDraw> = mesh
            .draws
            .iter()
            .map(|draw| SceneDraw {
                texture: draw.texture_id + 1, // 0 is reserved for the white texture
                start: draw.vertex_start as u32,
                count: draw.vertex_count as u32,
                sdf: false,
            })
            .collect();
        let mut vertices: Vec<OverlayVertex> = mesh
            .vertices
            .iter()
            .map(|vertex| sprite_vertex(vertex, viewport))
            .collect();
        // Screen-space UI (health bars, prompt text) draws last, on top。
        // 编辑器场景视图置 render_ui_overlay=false：UI 由 egui 在设计分辨率包围框内
        // 绘制布局框，跳过此 NDC 覆盖层避免冲突。
        if self.render_ui_overlay {
            self.push_ui_panels(&mut vertices, &mut draws, width, height)?;
            self.push_ui_images(&mut vertices, &mut draws, width, height)?;
            self.push_ui_grids(&mut vertices, &mut draws, width, height)?;
            self.push_ui_bars(&mut vertices, &mut draws, width, height)?;
            self.push_ui_text(&mut vertices, &mut draws, width, height)?;
            // Scene-node UI components (Text / Image / Button) drawn on very top.
            self.push_scene_ui(&mut vertices, &mut draws, width, height)?;
        }
        // UI 合批：相邻元素在顶点缓冲里连续，合并同纹理同 sdf 的 draw，减少
        // draw call（如一排同纹理方块/面板 → 一次绘制）。
        merge_adjacent_draws(&mut draws);
        // 每帧的 UI 指标不再打 INFO 日志（避免持续刷屏）；需要时由下方 debug 级输出。
        tracing::debug!(
            target: "render",
            "[render] mesh vertices={} draws={} paths={}",
            vertices.len(),
            draws.len(),
            self.texture_paths.len()
        );
        let font_atlas = if let Some(atlas) = &self.font_atlas {
            let atlas = atlas.lock().expect("font atlas lock");
            let version = atlas.version();
            let changed = version != self.last_font_version;
            self.last_font_version = version;
            Some(FontAtlasPixels {
                width: atlas.width(),
                height: atlas.height(),
                pixels: if changed {
                    atlas.pixels().to_vec()
                } else {
                    Vec::new()
                },
                version,
            })
        } else {
            None
        };
        Ok(SceneMesh {
            vertices,
            draws,
            paths: self.texture_paths.clone(),
            font_atlas,
        })
    }

    /// Pushes every active particle as a small white-texture quad in world
    /// space, so they sit with the scene and fade out with their color alpha.
    fn push_particles(&mut self) -> Result<(), String> {
        let particles = self.script.with_particles(|particles| particles.to_vec());
        if particles.is_empty() {
            return Ok(());
        }
        let texture = self.texture_id(WHITE_TEXTURE_PATH);
        for particle in particles {
            let sprite = Sprite::new(Vec2::new(particle.size, particle.size))
                .map_err(|error| error.to_string())?
                .with_tint([
                    particle.color[0],
                    particle.color[1],
                    particle.color[2],
                    particle.color[3] * particle.fade(),
                ])
                .map_err(|error| error.to_string())?;
            let transform = Transform::from_position(particle.position);
            self.sprite_batch
                .push(SpriteBatchItem::new(texture, 1, sprite, transform));
        }
        Ok(())
    }

    /// Appends Lua-created `ui.panel` quads using the white texture.
    fn push_ui_panels(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let panels = self.script.with_dynamic_ui(|panels, _, _| panels.clone());
        for panel in panels.values().filter(|panel| panel.visible) {
            let (origin, size) = resolve_ui_rect(
                panel.position,
                panel.size,
                panel.space,
                panel.anchor,
                [width.max(1) as f32, height.max(1) as f32],
                self.ui_reference,
            );
            let start = vertices.len() as u32;
            vertices.extend(screen_rect_ndc(
                origin[0],
                origin[1],
                size[0],
                size[1],
                panel.color,
                width,
                height,
            ));
            draws.push(SceneDraw {
                texture: 0, // white
                start,
                count: vertices.len() as u32 - start,
                sdf: false,
            });
        }
        Ok(())
    }

    /// Appends Lua-created `ui.image` quads above panels.
    fn push_ui_images(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let images = self.script.with_dynamic_ui(|_, images, _| images.clone());
        for image in images.values() {
            self.push_image_quad(vertices, draws, width, height, image)?;
        }
        Ok(())
    }

    /// Appends Lua-created `ui.grid` cells and their item icon textures.
    fn push_ui_grids(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let grids = self.script.with_dynamic_ui(|_, _, grids| grids.clone());
        for grid in grids.values() {
            let (origin, total) = resolve_ui_rect(
                grid.position,
                grid.total_size(),
                grid.space,
                grid.anchor,
                [width.max(1) as f32, height.max(1) as f32],
                self.ui_reference,
            );
            let start = vertices.len() as u32;
            vertices.extend(screen_rect_ndc(
                origin[0],
                origin[1],
                total[0],
                total[1],
                grid.background,
                width,
                height,
            ));
            draws.push(SceneDraw {
                texture: 0,
                start,
                count: vertices.len() as u32 - start,
                sdf: false,
            });
            for index in 1..=grid.columns * grid.rows {
                let Some((cell_origin, cell_size)) = grid.cell_rect(
                    index,
                    [width.max(1) as f32, height.max(1) as f32],
                    self.ui_reference,
                ) else {
                    continue;
                };
                let cell = grid.cells.get(index - 1).cloned().unwrap_or_default();
                let start = vertices.len() as u32;
                vertices.extend(screen_rect_ndc(
                    cell_origin[0],
                    cell_origin[1],
                    cell_size[0],
                    cell_size[1],
                    cell.color,
                    width,
                    height,
                ));
                draws.push(SceneDraw {
                    texture: 0,
                    start,
                    count: vertices.len() as u32 - start,
                    sdf: false,
                });
                if let Some(texture) = cell.texture {
                    let image = UiImage {
                        texture,
                        position: cell_origin,
                        size: cell_size,
                        color: [cell.color[0], cell.color[1], cell.color[2]],
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                        nine_slice: None,
                    };
                    self.push_image_quad(vertices, draws, width, height, &image)?;
                }
            }
        }
        Ok(())
    }

    /// Appends screen-space health bars (background + fill) as NDC quads using
    /// the white texture.
    fn push_ui_bars(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let bars = self.script.with_ui(|_, bars| bars.clone());
        for bar in bars.values() {
            let start = vertices.len() as u32;
            let background = [0.12, 0.12, 0.15, 0.75];
            let fill_width = (bar.size[0] * bar.ratio).max(0.0);
            vertices.extend_from_slice(&screen_rect_ndc(
                bar.position[0],
                bar.position[1],
                bar.size[0],
                bar.size[1],
                background,
                width,
                height,
            ));
            vertices.extend_from_slice(&screen_rect_ndc(
                bar.position[0],
                bar.position[1],
                fill_width,
                bar.size[1],
                bar.color,
                width,
                height,
            ));
            let count = vertices.len() as u32 - start;
            draws.push(SceneDraw {
                texture: 0, // white
                start,
                count,
                sdf: false,
            });
        }
        Ok(())
    }

    /// Appends screen-space text glyphs (SDF font atlas) as NDC quads. Shares
    /// `layout_text` with the scene-node UI Text component.
    fn push_ui_text(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let texts = self.script.with_ui(|texts, _| texts.clone());
        let visible: Vec<_> = texts
            .values()
            .filter(|text| text.visible)
            .map(|text| {
                (
                    text.position,
                    text.text.clone(),
                    ui_text_effective_scale(text.scale, text.font_size),
                )
            })
            .collect();
        if visible.is_empty() {
            return Ok(());
        }
        for (position, text, scale) in visible {
            let (mesh, is_sdf) = self.layout_text(&text, position, scale, [1.0, 1.0, 1.0])?;
            let text_vertices = text_to_ndc(mesh, is_sdf, width, height);
            self.push_text_run(vertices, draws, text_vertices, is_sdf);
        }
        Ok(())
    }

    /// Pushes scene-node UI components (`Text` / `Image` / `Button`) as NDC
    /// overlay quads on top of the Lua UI. Order: images, buttons, then text.
    /// Buttons render by composing the same image/text helpers used elsewhere.
    fn push_scene_ui(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let images: Vec<UiImage> = self.script.with_world(|world| {
            let mut images = Vec::new();
            world.for_each::<UiImage>(|_, image| images.push(image.clone()));
            images
        });
        for image in images {
            self.push_image_quad(vertices, draws, width, height, &image)?;
        }
        let texts: Vec<UiText> = self.script.with_world(|world| {
            let mut texts = Vec::new();
            world.for_each::<UiText>(|_, text| texts.push(text.clone()));
            texts
        });
        for text in texts {
            let viewport = [width.max(1) as f32, height.max(1) as f32];
            let effective_scale = text.effective_scale();
            let (_, resolved_scale) = resolve_ui_text_origin(
                text.position,
                [0.0, 0.0],
                effective_scale,
                text.space,
                text.anchor,
                viewport,
                self.ui_reference,
            );
            let (mesh, is_sdf) =
                self.layout_text(&text.text, [0.0, 0.0], resolved_scale, text.color)?;
            let (origin, _) = resolve_ui_text_origin(
                text.position,
                mesh.size,
                effective_scale,
                text.space,
                text.anchor,
                viewport,
                self.ui_reference,
            );
            let text_vertices = text_to_ndc_with_offset(mesh, is_sdf, origin, width, height);
            self.push_text_run(vertices, draws, text_vertices, is_sdf);
        }
        Ok(())
    }

    /// Layouts a string into a pixel-space text mesh using real font metrics.
    /// Uses the SDF font atlas when available, else the legacy bitmap atlas.
    /// Returns `(mesh, is_sdf)`.
    fn layout_text(
        &mut self,
        text: &str,
        position: [f32; 2],
        scale: f32,
        color: [f32; 3],
    ) -> Result<(TextMesh, bool), String> {
        self.ensure_font_atlas();
        let style = TextStyle {
            scale,
            color: [color[0], color[1], color[2], 1.0],
            ..Default::default()
        };
        if let Some(font) = &self.font_atlas {
            let mut font = font.lock().expect("font atlas lock");
            TextLayout::build(text, position, style, &mut font)
                .map(|mesh| (mesh, true))
                .map_err(|error| error.to_string())
        } else {
            let atlas = TextAtlas::built_in();
            TextLayout::build_bitmap(text, position, style, &atlas)
                .map(|mesh| (mesh, false))
                .map_err(|error| error.to_string())
        }
    }

    /// Appends a text run as a single SDF or bitmap draw.
    fn push_text_run(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        text_vertices: Vec<OverlayVertex>,
        is_sdf: bool,
    ) {
        if text_vertices.is_empty() {
            return;
        }
        let start = vertices.len() as u32;
        let texture = if is_sdf {
            self.texture_id(FONT_ATLAS_TEXTURE_PATH) + 1
        } else {
            self.texture_id(ATLAS_TEXTURE_PATH) + 1
        };
        vertices.extend(text_vertices);
        draws.push(SceneDraw {
            texture,
            start,
            count: vertices.len() as u32 - start,
            sdf: is_sdf,
        });
    }

    /// Pushes a screen-space textured quad for the `Image` component.
    fn push_image_quad(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
        image: &UiImage,
    ) -> Result<(), String> {
        let sw = width.max(1) as f32;
        let sh = height.max(1) as f32;
        let (origin, size) = resolve_ui_rect(
            image.position,
            image.size,
            image.space,
            image.anchor,
            [sw, sh],
            self.ui_reference,
        );
        let color = [image.color[0], image.color[1], image.color[2], 1.0];
        // 九宫格路径：把目标矩形拆成 3×3 格。无切片 / 纹理尺寸取不到时回退单四边形。
        if let Some(slice) = image
            .nine_slice
            .filter(|s| s.iter().all(|v| v.is_finite() && *v >= 0.0) && s.iter().any(|v| *v > 0.0))
        {
            if let Some((tw, th)) = self.texture_dimensions(&image.texture) {
                // 把作者空间边框映射到解析后的目标空间，使角部在 Canvas 缩放下
                // 保持纹理像素密度（Screen / scale=1 时比值为 1）。
                let scale_x = if image.size[0] > f32::EPSILON {
                    size[0] / image.size[0]
                } else {
                    1.0
                };
                let scale_y = if image.size[1] > f32::EPSILON {
                    size[1] / image.size[1]
                } else {
                    1.0
                };
                let slice = [
                    slice[0] * scale_x,
                    slice[1] * scale_x,
                    slice[2] * scale_y,
                    slice[3] * scale_y,
                ];
                let cells = nine_slice_cells(size[0], size[1], tw as f32, th as f32, slice);
                let start = vertices.len() as u32;
                for cell in cells {
                    let cw = cell.dest[2] - cell.dest[0];
                    let ch = cell.dest[3] - cell.dest[1];
                    if cw <= 0.0 || ch <= 0.0 {
                        continue;
                    }
                    let x0 = (origin[0] + cell.dest[0]) / sw * 2.0 - 1.0;
                    let y0 = 1.0 - (origin[1] + cell.dest[1]) / sh * 2.0;
                    let x1 = (origin[0] + cell.dest[2]) / sw * 2.0 - 1.0;
                    let y1 = 1.0 - (origin[1] + cell.dest[3]) / sh * 2.0;
                    vertices.extend(nine_slice_quad(x0, y0, x1, y1, cell.src, color));
                }
                if vertices.len() as u32 > start {
                    draws.push(SceneDraw {
                        texture: self.texture_id(&image.texture) + 1,
                        start,
                        count: vertices.len() as u32 - start,
                        sdf: false,
                    });
                }
                return Ok(());
            }
        }
        let x0 = origin[0] / sw * 2.0 - 1.0;
        let y0 = 1.0 - origin[1] / sh * 2.0;
        let x1 = (origin[0] + size[0]) / sw * 2.0 - 1.0;
        let y1 = 1.0 - (origin[1] + size[1]) / sh * 2.0;
        let quad = [
            OverlayVertex {
                position: [x0, y0],
                uv: [0.0, 0.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x1, y0],
                uv: [1.0, 0.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x1, y1],
                uv: [1.0, 1.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x0, y0],
                uv: [0.0, 0.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x1, y1],
                uv: [1.0, 1.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x0, y1],
                uv: [0.0, 1.0],
                color,
                sdf: 0,
            },
        ];
        let start = vertices.len() as u32;
        vertices.extend(quad);
        draws.push(SceneDraw {
            texture: self.texture_id(&image.texture) + 1,
            start,
            count: vertices.len() as u32 - start,
            sdf: false,
        });
        Ok(())
    }

    /// Deduplicates a texture asset path and returns its id for batch items.
    fn texture_id(&mut self, path: &str) -> u32 {
        if let Some(index) = self.texture_lookup.get(path) {
            return *index;
        }
        let index = self.texture_paths.len() as u32;
        let path = path.to_owned();
        self.texture_paths.push(path.clone());
        self.texture_lookup.insert(path, index);
        index
    }

    /// 纹理像素尺寸（惰性、按 path 缓存），仅供九宫格 UV 计算。只读 PNG 头，
    /// 不做全量解码。路径解析与渲染器一致（`project_root.join(path)`，无根则用
    /// 原始路径），并走与纹理加载相同的 pack/decode 解包。读取失败 → `None` →
    /// 回退单四边形。
    fn texture_dimensions(&mut self, path: &str) -> Option<(u32, u32)> {
        if let Some(dims) = self.texture_dims.get(path) {
            return Some(*dims);
        }
        let resolved = self
            .project_root
            .as_ref()
            .map(|root| root.join(path))
            .unwrap_or_else(|| PathBuf::from(path));
        let dims = fs::read(&resolved)
            .ok()
            .and_then(|bytes| crate::core::pack::decode(&bytes).ok())
            .and_then(|bytes| {
                image::ImageReader::new(std::io::Cursor::new(bytes))
                    .with_guessed_format()
                    .ok()?
                    .into_dimensions()
                    .ok()
            });
        if let Some(dims) = dims {
            self.texture_dims.insert(path.to_owned(), dims);
        }
        dims
    }

    /// Builds the 2D mesh for all Tilemap components using a tilemap-specific
    /// hot path that precomputes shared camera/rotation terms once per frame.
    fn build_tilemap_mesh(&mut self, viewport: Viewport) -> Result<SpriteBatchMesh, String> {
        let tilemaps = self.script.with_world(|world| {
            let mut tilemaps = Vec::new();
            world.for_each2::<Transform, Tilemap>(|id, transform, tilemap| {
                tilemaps.push((id, *transform, tilemap.clone()));
            });
            tilemaps
        });
        let mut next_cache = HashMap::with_capacity(tilemaps.len());
        let mut vertices = Vec::new();
        let mut draws = Vec::new();
        let camera = TilemapCamera::new(self.camera, viewport)?;
        for (entity, transform, tilemap) in tilemaps {
            let texture = self.texture_id(&tilemap.texture);
            let cached = self.tilemap_cache.remove(&entity);
            let layer = match cached {
                Some(layer) if layer.transform == transform && layer.tilemap == tilemap => layer,
                _ => build_cached_tilemap_layer(transform, tilemap.clone())?,
            };
            append_tilemap_layer(&mut vertices, &mut draws, texture, &camera, &layer);
            next_cache.insert(entity, layer);
        }
        self.tilemap_cache = next_cache;
        Ok(SpriteBatchMesh { vertices, draws })
    }
}

fn build_cached_tilemap_layer(
    transform: Transform,
    tilemap: Tilemap,
) -> Result<CachedTilemapLayer, String> {
    let mut sprites = Vec::new();
    for y in 0..tilemap.height {
        for x in 0..tilemap.width {
            let index = y * tilemap.width + x;
            let cell = tilemap.tiles[index];
            if cell == 0 {
                continue;
            }
            let (uv_min, uv_max) = Tilemap::tile_uv(cell);
            let center_offset = Vec2::new(
                (x as f32 + 0.5) * tilemap.tile_size.x * transform.scale.x,
                (y as f32 + 0.5) * tilemap.tile_size.y * transform.scale.y,
            );
            sprites.push(CachedTileSprite {
                center_offset,
                uv_min,
                uv_max,
            });
        }
    }
    Ok(CachedTilemapLayer {
        transform,
        tilemap,
        sprites,
    })
}

#[derive(Clone, Copy)]
struct TilemapCamera {
    center: Vec2,
    sin: f32,
    cos: f32,
    zoom: f32,
    half_width: f32,
    half_height: f32,
}

impl TilemapCamera {
    fn new(camera: Camera2D, viewport: Viewport) -> Result<Self, String> {
        let center = constrained_camera_center(camera, viewport);
        let (sin, cos) = camera.rotation.sin_cos();
        Ok(Self {
            center,
            sin,
            cos,
            zoom: camera.zoom,
            half_width: viewport.width * 0.5,
            half_height: viewport.height * 0.5,
        })
    }

    fn world_to_screen(self, world: Vec2) -> [f32; 2] {
        let delta = Vec2::new(world.x - self.center.x, world.y - self.center.y);
        let view = Vec2::new(
            delta.x * self.cos + delta.y * self.sin,
            -delta.x * self.sin + delta.y * self.cos,
        );
        [
            view.x * self.zoom + self.half_width,
            self.half_height - view.y * self.zoom,
        ]
    }

    fn world_offset_to_screen_delta(self, world: Vec2) -> [f32; 2] {
        let view = Vec2::new(
            world.x * self.cos + world.y * self.sin,
            -world.x * self.sin + world.y * self.cos,
        );
        [view.x * self.zoom, -view.y * self.zoom]
    }
}

fn constrained_camera_center(camera: Camera2D, viewport: Viewport) -> Vec2 {
    let Some(bounds) = camera.bounds else {
        return camera.center;
    };
    let half_width = viewport.width * 0.5 / camera.zoom;
    let half_height = viewport.height * 0.5 / camera.zoom;
    let (sin, cos) = camera.rotation.sin_cos();
    let extent_x = half_width * cos.abs() + half_height * sin.abs();
    let extent_y = half_width * sin.abs() + half_height * cos.abs();
    Vec2::new(
        clamp_center(
            camera.center.x,
            bounds.min.x + extent_x,
            bounds.max.x - extent_x,
        ),
        clamp_center(
            camera.center.y,
            bounds.min.y + extent_y,
            bounds.max.y - extent_y,
        ),
    )
}

fn clamp_center(value: f32, minimum: f32, maximum: f32) -> f32 {
    if minimum > maximum {
        (minimum + maximum) * 0.5
    } else {
        value.clamp(minimum, maximum)
    }
}

fn append_tilemap_layer(
    vertices: &mut Vec<SpriteVertex>,
    draws: &mut Vec<SpriteDrawBatch>,
    texture_id: u32,
    camera: &TilemapCamera,
    layer: &CachedTilemapLayer,
) {
    if layer.sprites.is_empty() {
        return;
    }
    let start = vertices.len();
    let half = Vec2::new(
        0.5 * layer.tilemap.tile_size.x * layer.transform.scale.x,
        0.5 * layer.tilemap.tile_size.y * layer.transform.scale.y,
    );
    let (sin, cos) = layer.transform.rotation.sin_cos();
    let corners = [
        Vec2::new(-half.x, half.y),
        Vec2::new(half.x, half.y),
        Vec2::new(half.x, -half.y),
        Vec2::new(-half.x, -half.y),
    ];
    let screen_offsets = corners.map(|corner| {
        let rotated = Vec2::new(
            corner.x * cos - corner.y * sin,
            corner.x * sin + corner.y * cos,
        );
        camera.world_offset_to_screen_delta(rotated)
    });
    for tile in &layer.sprites {
        let center_world = Vec2::new(
            layer.transform.position.x + tile.center_offset.x,
            layer.transform.position.y + tile.center_offset.y,
        );
        let center = camera.world_to_screen(center_world);
        let positions = screen_offsets.map(|offset| [center[0] + offset[0], center[1] + offset[1]]);
        let uv = [
            [tile.uv_min.x, tile.uv_min.y],
            [tile.uv_max.x, tile.uv_min.y],
            [tile.uv_max.x, tile.uv_max.y],
            [tile.uv_min.x, tile.uv_max.y],
        ];
        let tint = [1.0; 4];
        vertices.extend_from_slice(&[
            SpriteVertex {
                position: positions[0],
                uv: uv[0],
                color: tint,
            },
            SpriteVertex {
                position: positions[1],
                uv: uv[1],
                color: tint,
            },
            SpriteVertex {
                position: positions[2],
                uv: uv[2],
                color: tint,
            },
            SpriteVertex {
                position: positions[0],
                uv: uv[0],
                color: tint,
            },
            SpriteVertex {
                position: positions[2],
                uv: uv[2],
                color: tint,
            },
            SpriteVertex {
                position: positions[3],
                uv: uv[3],
                color: tint,
            },
        ]);
    }
    let count = vertices.len() - start;
    if let Some(previous) = draws.last_mut()
        && previous.texture_id == texture_id
        && previous.layer == 0
        && previous.vertex_start + previous.vertex_count == start
    {
        previous.vertex_count += count;
    } else {
        draws.push(SpriteDrawBatch {
            texture_id,
            layer: 0,
            vertex_start: start,
            vertex_count: count,
        });
    }
}

fn merge_sprite_meshes(tilemap: SpriteBatchMesh, dynamic: SpriteBatchMesh) -> SpriteBatchMesh {
    if tilemap.vertices.is_empty() {
        return dynamic;
    }
    if dynamic.vertices.is_empty() {
        return tilemap;
    }
    let base = tilemap.vertices.len();
    let mut vertices = tilemap.vertices;
    vertices.extend(dynamic.vertices);
    let mut draws = tilemap.draws;
    draws.extend(dynamic.draws.into_iter().map(|draw| SpriteDrawBatch {
        texture_id: draw.texture_id,
        layer: draw.layer,
        vertex_start: draw.vertex_start + base,
        vertex_count: draw.vertex_count,
    }));
    SpriteBatchMesh { vertices, draws }
}

fn startup_scene_path(project_root: &Path) -> String {
    let default = "scenes/main.scene".to_owned();
    let Ok(text) = fs::read_to_string(project_root.join("project.toml")) else {
        return default;
    };
    extract_project_key(&text, "startup_scene").unwrap_or(default)
}

/// project.toml 的 `font` 键：相对项目根的字形文件路径（如
/// `assets/fonts/msyh.ttc`）。缺省回退 NotoSansSC-VF.ttf。
fn project_font_path(project_root: &Path) -> Option<String> {
    let text = fs::read_to_string(project_root.join("project.toml")).ok()?;
    extract_project_key(&text, "font")
}

/// project.toml 的 `font_fallback` 键：后备字形文件路径（主字体缺失的字形用）。
fn project_fallback_font_path(project_root: &Path) -> Option<String> {
    let text = fs::read_to_string(project_root.join("project.toml")).ok()?;
    extract_project_key(&text, "font_fallback")
}

/// 加载项目字体并包装为可跨运行时共享的图集（MSDF 字形按需生成，共享后只生成
/// 一次）。字体路径取 project.toml `font` 键，缺省 NotoSansSC-VF.ttf。
pub fn load_project_font_atlas(project_root: Option<&Path>) -> Option<Arc<Mutex<FontAtlas>>> {
    let root = project_root?;
    let primary_path = {
        let font =
            project_font_path(root).unwrap_or_else(|| "assets/fonts/NotoSansSC-VF.ttf".to_owned());
        let path = root.join(font);
        path.is_file().then_some(path)
    }?;
    let primary_bytes = fs::read(&primary_path).ok()?;
    let primary_bytes = crate::core::pack::decode(&primary_bytes).ok()?;
    // 可选后备字体（project.toml `font_fallback` 键），用于主字体缺失的字形。
    let fallback_bytes = project_fallback_font_path(root)
        .and_then(|font| {
            let path = root.join(font);
            path.is_file().then_some(path)
        })
        .and_then(|path| {
            let bytes = fs::read(&path).ok()?;
            crate::core::pack::decode(&bytes).ok()
        });
    let mut atlas = match FontAtlas::load_with_fallback(&primary_bytes, fallback_bytes.as_deref()) {
        Ok(atlas) => atlas,
        Err(error) => {
            tracing::warn!(target: "render", error = %error, "[render] font atlas load failed; text uses placeholders");
            return None;
        }
    };
    tracing::info!(target: "render", path = %primary_path.display(), "[render] font atlas ready");
    // 首包预热：把打包时收集的字符清单（chars.txt）预生成进图集，首帧文字不卡。
    prewarm_font_atlas(root, &mut atlas);
    Some(Arc::new(Mutex::new(atlas)))
}

/// 从项目根 `chars.txt`（打包时由 assetc 生成）读取字符并预热字形。
/// MSDF 光栅化跨线程并行，debug 构建下显著加速（ui_demo 135 字从 ~2.3s 降到 ~0.3s）。
fn prewarm_font_atlas(root: &Path, atlas: &mut FontAtlas) {
    let Ok(bytes) = fs::read(root.join("chars.txt")) else {
        return;
    };
    let Ok(bytes) = crate::core::pack::decode(&bytes) else {
        return;
    };
    let text = String::from_utf8_lossy(&bytes);
    let count = atlas.prewarm(text.chars());
    if count > 0 {
        tracing::info!(target: "render", count, "[render] font prewarmed {count} chars");
    }
}

fn extract_project_key(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim().trim_start_matches('\u{feff}');
        line.strip_prefix(&format!("{key} ="))
            .map(|value| value.trim().trim_matches('"').to_owned())
            .filter(|value| !value.trim().is_empty())
    })
}

/// Sprite 的世界矩形（pivot+缩放+旋转+位移）投影到屏幕的 AABB，供 clickEvent 命中。
fn sprite_screen_rect(
    sprite: &Sprite,
    transform: &Transform,
    camera: Camera2D,
    viewport: Viewport,
) -> Option<([f32; 2], [f32; 2])> {
    // 局部四角：(-pivot*size) 到 ((1-pivot)*size)，经缩放/旋转/位移后投影。
    let pivot = sprite.pivot;
    let size = sprite.size;
    let local = [
        Vec2::new(-pivot.x * size.x, -pivot.y * size.y),
        Vec2::new((1.0 - pivot.x) * size.x, -pivot.y * size.y),
        Vec2::new((1.0 - pivot.x) * size.x, (1.0 - pivot.y) * size.y),
        Vec2::new(-pivot.x * size.x, (1.0 - pivot.y) * size.y),
    ];
    let (sin, cos) = transform.rotation.sin_cos();
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for corner in local {
        let scaled = Vec2::new(corner.x * transform.scale.x, corner.y * transform.scale.y);
        let rotated = Vec2::new(
            scaled.x * cos - scaled.y * sin,
            scaled.x * sin + scaled.y * cos,
        );
        let world = Vec2::new(
            transform.position.x + rotated.x,
            transform.position.y + rotated.y,
        );
        let screen = camera.world_to_screen(world, viewport).ok()?;
        min[0] = min[0].min(screen.x);
        min[1] = min[1].min(screen.y);
        max[0] = max[0].max(screen.x);
        max[1] = max[1].max(screen.y);
    }
    Some((min, [max[0] - min[0], max[1] - min[1]]))
}

fn load_runtime_scene(project_root: Option<&Path>) -> Scene {
    project_root
        .and_then(|root| {
            let scene_path = startup_scene_path(root);
            Scene::load(root.join(scene_path)).ok()
        })
        // Standalone engine mode uses this engine-owned diagnostic scene and never
        // probes the current directory for game files.
        .unwrap_or_else(|| {
            let mut scene = Scene::new("runtime-main");
            let _ = scene.add_entity(SceneEntityId(1), "SpriteRoot", None);
            scene
                .entities
                .get_mut(&SceneEntityId(1))
                .unwrap()
                .components
                .insert(
                    "Transform".into(),
                    "x=640;y=360;rotation=0;scale_x=1;scale_y=1".into(),
                );
            scene
                .entities
                .get_mut(&SceneEntityId(1))
                .unwrap()
                .components
                .insert("Sprite".into(), "assets/textures/red-1x1.png".into());
            scene
        })
}

/// UI 合批：合并相邻且同纹理、同 sdf 的 draw。
///
/// 各元素在 `vertices()` 中按顺序追加到同一个顶点缓冲，因此相邻 draw 的顶点区间
/// 是连续的——把 `count` 合并到前一条即可，渲染结果不变（同纹理同管线，绘制顺序
/// 保持）。这能把一排同纹理的 UI 方块/面板从 N 次 draw call 降为 1 次。
fn merge_adjacent_draws(draws: &mut Vec<SceneDraw>) {
    let mut out: Vec<SceneDraw> = Vec::with_capacity(draws.len());
    for draw in draws.drain(..) {
        if let Some(last) = out.last_mut()
            && last.texture == draw.texture
            && last.sdf == draw.sdf
        {
            // 相邻 draw 顶点连续（start == last.start + last.count），直接并 count。
            last.count += draw.count;
            continue;
        }
        out.push(draw);
    }
    *draws = out;
}

fn sprite_vertex(vertex: &SpriteVertex, viewport: Viewport) -> OverlayVertex {
    OverlayVertex {
        position: [
            vertex.position[0] / viewport.width * 2.0 - 1.0,
            1.0 - vertex.position[1] / viewport.height * 2.0,
        ],
        uv: vertex.uv,
        color: vertex.color,
        sdf: 0,
    }
}

/// Converts a pixel-space text mesh into NDC overlay vertices.
fn text_to_ndc(mesh: TextMesh, is_sdf: bool, width: u32, height: u32) -> Vec<OverlayVertex> {
    text_to_ndc_with_offset(mesh, is_sdf, [0.0, 0.0], width, height)
}

/// Converts a pixel-space text mesh into NDC overlay vertices with an extra
/// pixel offset (used to center a button label).
fn text_to_ndc_with_offset(
    mesh: TextMesh,
    is_sdf: bool,
    offset: [f32; 2],
    width: u32,
    height: u32,
) -> Vec<OverlayVertex> {
    mesh.vertices
        .into_iter()
        .map(|vertex| OverlayVertex {
            position: [
                (vertex.position[0] + offset[0]) / width.max(1) as f32 * 2.0 - 1.0,
                1.0 - (vertex.position[1] + offset[1]) / height.max(1) as f32 * 2.0,
            ],
            uv: vertex.uv,
            color: vertex.color,
            sdf: u32::from(is_sdf),
        })
        .collect()
}

/// Builds a quad in NDC from a top-left-origin screen-space rectangle, sampling
/// the white texture with the given flat color.
/// 6-vertex textured quad in NDC with a per-cell UV rect (nine-slice cell).
/// Winding/UV order mirrors the single-quad code in `push_image_quad`.
fn nine_slice_quad(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    uv: [f32; 4],
    color: [f32; 4],
) -> [OverlayVertex; 6] {
    let (u0, v0, u1, v1) = (uv[0], uv[1], uv[2], uv[3]);
    [
        OverlayVertex {
            position: [x0, y0],
            uv: [u0, v0],
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x1, y0],
            uv: [u1, v0],
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x1, y1],
            uv: [u1, v1],
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x0, y0],
            uv: [u0, v0],
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x1, y1],
            uv: [u1, v1],
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x0, y1],
            uv: [u0, v1],
            color,
            sdf: 0,
        },
    ]
}

fn screen_rect_ndc(
    x: f32,
    y: f32,
    rect_w: f32,
    rect_h: f32,
    color: [f32; 4],
    screen_width: u32,
    screen_height: u32,
) -> [OverlayVertex; 6] {
    let sw = screen_width.max(1) as f32;
    let sh = screen_height.max(1) as f32;
    let x0 = x / sw * 2.0 - 1.0;
    let y0 = 1.0 - y / sh * 2.0;
    let x1 = (x + rect_w) / sw * 2.0 - 1.0;
    let y1 = 1.0 - (y + rect_h) / sh * 2.0;
    let uv = [0.0, 0.0];
    [
        OverlayVertex {
            position: [x0, y0],
            uv,
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x1, y0],
            uv,
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x1, y1],
            uv,
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x0, y0],
            uv,
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x1, y1],
            uv,
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x0, y1],
            uv,
            color,
            sdf: 0,
        },
    ]
}

#[cfg(test)]
mod sdf_tests {
    use super::*;
    use crate::core::scene::Scene;

    fn game_root() -> std::path::PathBuf {
        crate::core::games_dir().join("game_test")
    }

    fn production_sample_root() -> std::path::PathBuf {
        crate::core::games_dir().join("game_test_2d_production")
    }

    fn farm_root() -> std::path::PathBuf {
        crate::core::games_dir().join("game_farm")
    }

    #[test]
    fn farm_sample_runs_with_new_api() {
        let root = farm_root();
        if !root.is_dir() {
            return;
        }
        let mut runtime = RuntimeScene::new(Some(&root)).unwrap();
        runtime
            .run_script_steps(2, 1.0 / 60.0, &crate::core::input::InputSnapshot::default())
            .unwrap();
        let has_hud = runtime
            .script
            .with_ui(|texts, _| texts.contains_key("status"));
        assert!(has_hud, "farm HUD should be created every update");
    }

    /// ui_demo 测试工程：场景渲染出 UI/世界 draw、clickEvent 目标存在，且点击
    /// StartButton 会经 main.lua 的 on_click 分发（update 返回带实体名）。
    #[test]
    fn ui_demo_project_renders_ui_and_dispatches_clicks() {
        use crate::core::input::MouseButton;
        use crate::core::scene::Scene;

        let root = crate::core::games_dir().join("ui_demo");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let path = root.join("scenes/main.scene");
        assert!(path.is_file(), "ui_demo 场景缺失: {}", path.display());
        let scene = Scene::load(&path).unwrap();
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene).unwrap();
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        assert!(!mesh.draws.is_empty(), "ui_demo 应渲染出可见 draw");
        let targets = runtime.compute_click_targets();
        assert!(
            targets.len() >= 5,
            "ui_demo 应有多个 clickEvent 目标: {}",
            targets.len()
        );
        let start = targets
            .iter()
            .find(|(n, _, _)| n == "StartButton")
            .expect("StartButton clickEvent 目标");
        let cx = start.1[0] + start.2[0] / 2.0;
        let cy = start.1[1] + start.2[1] / 2.0;
        let mut input = InputSnapshot::default();
        input.cursor_moved(cx as f64, cy as f64);
        input.mouse_event(MouseButton::Left, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("script step");
        let result = runtime.script.call_update().unwrap();
        assert!(
            result.contains("StartButton"),
            "点击应分发 on_click('StartButton'): {result}"
        );
    }

    /// 九宫格：带 `nine_slice` 的 Image 应渲染为 3×3 共 9 格（54 顶点），而非
    /// 单四边形（6 顶点）。用 ui_demo 真实按钮纹理（192×64）验证 UV 分片路径。
    #[test]
    fn nine_slice_image_renders_nine_quads() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = crate::core::games_dir().join("ui_demo");
        if !root.join("assets/textures/ui/button_green.png").is_file() {
            return;
        }
        let mut scene = Scene::new("nine-slice");
        scene.add_entity(SceneEntityId(1), "Panel", None).unwrap();
        scene.entities.get_mut(&SceneEntityId(1)).unwrap().components.insert(
            "Image".to_owned(),
            "texture=assets/textures/ui/button_green.png;x=0;y=0;width=192;height=64;color=1,1,1;space=screen;anchor=0,0;nine_slice=16,16,16,16"
                .to_owned(),
        );
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        assert!(
            mesh.draws.iter().any(|d| d.count == 54),
            "切片 Image 应产生 54 顶点（9 格 × 6）：{:#?}",
            mesh.draws.iter().map(|d| d.count).collect::<Vec<_>>()
        );
    }

    /// Canvas UI 的渲染位置随 viewport/reference 缩放。场景视图切换设计分辨率时
    /// 必须同步 `ui_reference`（`render_scene_screen` 已修复）：用旧 reference 渲染
    /// 会让 UI 与包围框（scale=1）错位。这里验证同步后 StartButton 落在
    /// 1920×1080 设计位置。
    #[test]
    fn canvas_ui_follows_ui_reference_on_resolution_change() {
        use crate::core::scene::Scene;

        let root = crate::core::games_dir().join("ui_demo");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let scene = Scene::load(&root.join("scenes/main.scene")).unwrap();
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene).unwrap();
        // 旧参考分辨率（运行时构建时的 1280×720）。
        runtime.set_ui_reference([1280.0, 720.0]);
        runtime.vertices(1920, 1080).expect("vertices");
        let rect = |runtime: &mut RuntimeScene| -> ([f32; 2], [f32; 2]) {
            let targets = runtime.compute_click_targets();
            let (_, origin, size) = targets
                .iter()
                .find(|(n, _, _)| n == "StartButton")
                .expect("StartButton click target");
            (*origin, *size)
        };
        // 修复前（不同步 ui_reference）：viewport=1920×1080、reference=1280×720 →
        // 画布缩放 1.5×，UI 位置/尺寸错误。
        let (stale_origin, stale_size) = rect(&mut runtime);
        // 修复后：同步 reference 到当前设计分辨率 → scale=1，落在设计位置。
        runtime.set_ui_reference([1920.0, 1080.0]);
        let (origin, size) = rect(&mut runtime);
        // StartButton：canvas 位置 [-240,20]，尺寸 192×64，anchor 0.5,0.5 →
        // 1920×1080 下中心 (720,560)，origin (624,528)。
        assert_eq!(origin, [624.0, 528.0], "同步 reference 后应在设计位置");
        assert_eq!(size, [192.0, 64.0]);
        assert_ne!(
            stale_origin, origin,
            "旧 reference 会导致 UI 错位（回归防护）"
        );
        let _ = stale_size;
    }

    /// 首包预热：`load_project_font_atlas` 读到项目根 `chars.txt` 时预生成字形
    /// （图集 version 增加）；无 chars.txt 不预热。
    #[test]
    fn font_prewarm_from_chars_txt() {
        let root = std::env::temp_dir().join(format!("p2_font_prewarm_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let font_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/editor/fonts/NotoSansSC-VF.ttf");
        std::fs::create_dir_all(root.join("assets/fonts")).unwrap();
        std::fs::copy(&font_src, root.join("assets/fonts/font.ttf")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nfont = \"assets/fonts/font.ttf\"\n",
        )
        .unwrap();
        // 无 chars.txt：不预热，version 保持 0。
        let atlas = load_project_font_atlas(Some(&root)).expect("atlas");
        assert_eq!(atlas.lock().unwrap().version(), 0, "无 chars.txt 不应预热");
        // 写 chars.txt：预热后 version > 0（字形已生成）。
        std::fs::write(root.join("chars.txt"), "测试字体AB").unwrap();
        let atlas = load_project_font_atlas(Some(&root)).expect("atlas");
        assert!(atlas.lock().unwrap().version() > 0, "chars.txt 应触发预热");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 俄罗斯方块工程冒烟：加载场景 + 脚本，跑几步（输入驱动），验证精灵/UI
    /// 渲染与 Lua 脚本不报错。
    #[test]
    fn tetris_project_runs_and_renders() {
        use crate::core::input::InputKey;
        use crate::core::scene::Scene;

        let root = crate::core::games_dir().join("tetris");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let path = root.join("scenes/main.scene");
        assert!(path.is_file(), "tetris 场景缺失: {}", path.display());
        assert!(root.join("scripts/main.lua").is_file(), "tetris 脚本缺失");
        let scene = Scene::load(&path).unwrap();
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene).unwrap();
        // on_create 已执行：背景面板 + 首块精灵 + 文字应渲染出 draw。
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        assert!(
            !mesh.draws.is_empty(),
            "tetris 应渲染出 draw（UI 面板/精灵/文字）"
        );
        // 关键：必须存在 white.png 的精灵 draw（方块），否则只渲染了 UI。
        let has_sprite_draw = mesh.draws.iter().any(|d| {
            d.texture > 0
                && mesh.paths.get(d.texture as usize - 1).map(|p| p.as_str())
                    == Some("assets/textures/white.png")
        });
        assert!(
            has_sprite_draw,
            "应存在 white.png 精灵 draw（方块）：{:#?}",
            mesh.paths
        );
        // UI 合批：全部方块用同一纹理（white.png），应被合并成极少量 draw。
        let white_draws = mesh
            .draws
            .iter()
            .filter(|d| {
                d.texture > 0
                    && mesh.paths.get(d.texture as usize - 1).map(|p| p.as_str())
                        == Some("assets/textures/white.png")
            })
            .count();
        assert!(
            white_draws <= 2,
            "同纹理 UI 方块应合批为 ≤2 个 draw（初始 4 块），实际 {white_draws}"
        );
        // 模拟按键：右移 → 旋转 → 硬降，逐帧驱动。
        let mut input = InputSnapshot::default();
        input.key_event(InputKey::D, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step d");
        input.key_event(InputKey::W, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step w");
        input.key_event(InputKey::Space, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step space");
        // 多步后仍能渲染。
        let mesh = runtime.vertices(1280, 720).expect("vertices after steps");
        assert!(!mesh.draws.is_empty(), "多步后仍应渲染");
    }

    /// clickEvent 命中矩形：Image 用 UI 矩形、Text 用 SDF 字形 bbox、Sprite 用
    /// 相机投影到屏幕的 AABB。验证三类实体都能得到正确的 `(名字, origin, size)`。
    #[test]
    fn compute_click_targets_resolves_image_text_sprite() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = crate::core::games_dir().join("game_test");
        let mut scene = Scene::new("click");
        let add = |scene: &mut Scene, id: u64, name: &str, components: &[(&str, &str)]| {
            scene.add_entity(SceneEntityId(id), name, None).unwrap();
            let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
            for (key, value) in components {
                entity.components.insert(key.to_string(), value.to_string());
            }
        };
        add(
            &mut scene,
            1,
            "ImgClick",
            &[
                (
                    "Image",
                    "texture=assets/textures/coin.png;x=10;y=20;width=64;height=32;color=1,1,1;space=screen;anchor=0,0",
                ),
                ("clickEvent", ""),
            ],
        );
        add(
            &mut scene,
            2,
            "TxtClick",
            &[
                (
                    "Text",
                    "text=Go;x=5;y=5;scale=1;color=1,1,1;space=screen;anchor=0,0",
                ),
                ("clickEvent", ""),
            ],
        );
        add(
            &mut scene,
            3,
            "SprClick",
            &[
                ("Transform", "x=100;y=50;rotation=0;scale_x=1;scale_y=1"),
                (
                    "Sprite",
                    "texture=assets/textures/coin.png;size=32x32;pivot=0.5,0.5;tint=1,1,1,1",
                ),
                ("clickEvent", ""),
            ],
        );
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
        runtime.set_camera_2d(Camera2D::new(Vec2::new(0.0, 0.0), 1.0).expect("default camera"));
        let targets = runtime.compute_click_targets();
        let img = targets
            .iter()
            .find(|(n, _, _)| n == "ImgClick")
            .expect("image click target");
        assert_eq!(img.1, [10.0, 20.0], "Image origin 即组件 x/y");
        assert_eq!(img.2, [64.0, 32.0], "Image size 即组件宽高");
        assert!(
            targets.iter().any(|(n, _, _)| n == "TxtClick"),
            "Text click target 存在"
        );
        let spr = targets
            .iter()
            .find(|(n, _, _)| n == "SprClick")
            .expect("sprite click target");
        // 相机在原点、viewport 1280×720：sprite(100,50) 32×32 pivot 0.5
        // → 屏幕 AABB 左上 (724, 294)、尺寸 (32, 32)。
        assert_eq!(spr.1, [724.0, 294.0]);
        assert_eq!(spr.2, [32.0, 32.0]);
    }

    #[test]
    fn load_runtime_scene_uses_project_startup_scene() {
        let root = std::env::temp_dir().join(format!("p2-startup-scene-{}", std::process::id()));
        let scenes = root.join("scenes");
        std::fs::create_dir_all(&scenes).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nname = \"test\"\nstartup_scene = \"scenes/custom.scene\"\n",
        )
        .unwrap();
        Scene::new("custom-startup")
            .save(scenes.join("custom.scene"))
            .unwrap();
        let scene = load_runtime_scene(Some(&root));
        assert_eq!(scene.name, "custom-startup");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ui_test_scene_emits_sdf_text_draws() {
        let root = game_root();
        if !root.join("scenes/ui_test.scene").is_file() {
            return;
        }
        let path = root.join("scenes/ui_test.scene");
        assert!(path.is_file(), "测试场景资源缺失: {}", path.display());
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load ui_test: {e}"));
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime");
        let mesh = runtime.vertices(640, 360).expect("vertices");
        let sdf_count = mesh.draws.iter().filter(|d| d.sdf).count();
        eprintln!(
            "ui_test: draws={} sdf={} text_entities={}",
            mesh.draws.len(),
            sdf_count,
            runtime
                .script
                .with_world(|w| w.entities_with::<UiText>().len())
        );
        assert!(sdf_count > 0, "ui_test scene should emit SDF text draws");
        assert!(
            mesh.vertices.iter().any(|v| v.sdf == 1),
            "SDF vertices present"
        );
    }

    /// 运行态 3D drawable 使用序列化场景的实体名称（而非 `Entity {index}`）；
    /// 未命名的脚本实体才回退到索引名。
    #[test]
    fn runtime_drawables_use_scene_names() {
        let root = game_root();
        if !root.join("scenes/demo3d.scene").is_file() {
            return;
        }
        let path = root.join("scenes/demo3d.scene");
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load demo3d: {e}"));
        let runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime demo3d");
        let scene3d = runtime.scene_3d(16.0 / 9.0).expect("3d scene");
        let mut names: Vec<&str> = scene3d.drawables.iter().map(|d| d.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["Ground", "ShadowTriangle", "SkeletonWarrior"]);
        assert!(
            scene3d
                .drawables
                .iter()
                .all(|d| !d.name.starts_with("Entity ")),
            "drawable 名称应来自场景实体名，而非索引"
        );
    }

    #[test]
    fn production_sample_menu_scene_instantiates_ui() {
        let root = production_sample_root();
        if !root.join("scenes/menu.scene").is_file() {
            return;
        }
        let path = root.join("scenes/menu.scene");
        assert!(path.is_file(), "样板菜单场景缺失: {}", path.display());
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load menu: {e}"));
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime menu");
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        let counts = runtime.script.with_world(|world| {
            (
                world.entities_with::<crate::core::ui::ClickEvent>().len(),
                world.entities_with::<UiImage>().len(),
                world.entities_with::<UiText>().len(),
            )
        });
        assert_eq!(counts.0, 2, "menu scene buttons are clickEvent entities");
        assert!(
            counts.1 >= 3,
            "menu scene should expose logo + two button images"
        );
        assert!(
            counts.2 >= 3,
            "menu scene should expose title/subtitle/help text"
        );
        assert!(
            !mesh.draws.is_empty(),
            "menu scene should produce visible 2D/UI draws"
        );
    }

    #[test]
    fn production_sample_scene_instantiates_gameplay_entities() {
        let root = production_sample_root();
        if !root.join("scenes/production.scene").is_file() {
            return;
        }
        let path = root.join("scenes/production.scene");
        assert!(path.is_file(), "样板生产场景缺失: {}", path.display());
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load production: {e}"));
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime production");
        let input = InputSnapshot::default();
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("script step");
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        let counts = runtime.script.with_world(|world| {
            (
                world.entities_with::<crate::core::tilemap::Tilemap>().len(),
                world
                    .entities_with::<crate::core::animation::AnimationStateMachine>()
                    .len(),
                world.entities_with::<crate::core::ui::ClickEvent>().len(),
            )
        });
        assert_eq!(counts.0, 1, "production scene should have one tilemap");
        assert_eq!(
            counts.1, 11,
            "production scene should have player plus ten animated enemies"
        );
        assert_eq!(
            counts.2, 0,
            "production scene (not migrated) has no clickEvent"
        );
        assert!(
            runtime.texture_paths.len() >= 4,
            "production scene should load tilemap/player/enemy/pickup textures"
        );
        assert!(
            mesh.draws.len() >= 4,
            "production scene should render tilemap and gameplay actors"
        );
    }

    /// 同一输入 scene 在静态 serialized extractor 与初始 runtime world 上：
    /// 每个 serialized 3D 实体都能找到同名、同 mesh path、同 transform 的
    /// runtime drawable；活动相机/光源与 serialized 的首个相机/光源一致
    /// （无 Lua 时 runtime 取首个组件实体）。物理/脚本/动画 palette 属运行态
    /// 差异，不在本测试断言逐帧相等。
    #[test]
    fn serialized_and_runtime_consistency_demo3d() {
        let root = game_root();
        if !root.join("scenes/demo3d.scene").is_file() {
            return;
        }
        let path = root.join("scenes/demo3d.scene");
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load demo3d: {e}"));
        let serialized = crate::core::scene_extract::extract_serialized_scene_3d(&scene);
        let runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime demo3d");
        let runtime3d = runtime.scene_3d(16.0 / 9.0).expect("3d scene");

        // serialized extractor 应有可断言的有效实体；合法性一致时无诊断。
        assert_eq!(serialized.entities.len(), runtime3d.drawables.len());
        assert!(serialized.diagnostics.is_empty());
        for serialized_entity in &serialized.entities {
            let drawable = runtime3d
                .drawables
                .iter()
                .find(|d| d.name == serialized_entity.name)
                .unwrap_or_else(|| panic!("runtime 缺少同名实体 {}", serialized_entity.name));
            assert_eq!(
                drawable.mesh_path, serialized_entity.mesh.mesh,
                "{} 的 mesh path 应一致",
                serialized_entity.name
            );
            for (row, col) in (0..4).flat_map(|r| (0..4).map(move |c| (r, c))) {
                let runtime_value = drawable.model.columns[col][row];
                let serialized_value = serialized_entity.model.columns[col][row];
                assert!(
                    (runtime_value - serialized_value).abs() < 1e-4,
                    "{} model[{col}][{row}] 不一致: {runtime_value} vs {serialized_value}",
                    serialized_entity.name
                );
            }
        }

        // 活动相机 = 首个 Camera3D 实体（from_scene_edit 无 Lua 选择），
        // 与 serialized 首个相机一致，仅 aspect_ratio 被 host 覆盖为传入值。
        assert!(!serialized.cameras.is_empty(), "demo3d 应有相机");
        let serialized_camera = &serialized.cameras[0].1;
        let runtime_camera = &runtime3d.camera;
        assert_eq!(runtime_camera.position, serialized_camera.position);
        assert_eq!(runtime_camera.target, serialized_camera.target);
        assert_eq!(runtime_camera.up, serialized_camera.up);
        assert!((runtime_camera.fov_y - serialized_camera.fov_y).abs() < 1e-6);
        assert!((runtime_camera.near - serialized_camera.near).abs() < 1e-6);
        assert!((runtime_camera.far - serialized_camera.far).abs() < 1e-6);
        assert!(
            (runtime_camera.aspect_ratio - 16.0 / 9.0).abs() < 1e-6,
            "runtime 相机 aspect 应为传入值"
        );

        // 活动光源 = 首个 Light 实体，与 serialized 首个光源一致。
        assert!(!serialized.lights.is_empty(), "demo3d 应有光源");
        let serialized_light = &serialized.lights[0].1;
        let runtime_light = &runtime3d.light;
        assert_eq!(runtime_light.direction, serialized_light.direction);
        assert_eq!(runtime_light.color, serialized_light.color);
        assert_eq!(runtime_light.intensity, serialized_light.intensity);
    }

    /// M11-04 / R-05：编辑器 3D 视图相机覆盖（`scene_3d_with_camera`）不修改场景
    /// 数据——传 `Some` 仅覆盖渲染视角（aspect 同步为传入值），传 `None` 走
    /// `active_camera3d`；覆盖前后 drawables 一致，且覆盖后 `scene_3d` 仍回退到
    /// 场景相机（证明覆盖是瞬时的视图参数，未写入 ECS / 脚本状态）。
    #[test]
    fn scene_3d_with_camera_override_keeps_scene_data_intact() {
        let root = game_root();
        if !root.join("scenes/demo3d.scene").is_file() {
            return;
        }
        let path = root.join("scenes/demo3d.scene");
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load demo3d: {e}"));
        let runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime demo3d");

        let before = runtime.scene_3d(16.0 / 9.0).expect("3d scene");
        let override_camera = Camera3D {
            position: Vec3::new(10.0, 20.0, 30.0),
            target: Vec3::new(-5.0, -6.0, -7.0),
            up: Vec3::Z,
            fov_y: 0.9,
            aspect_ratio: 2.0,
            near: 0.5,
            far: 200.0,
        };
        let overridden = runtime
            .scene_3d_with_camera(4.0 / 3.0, Some(override_camera))
            .expect("3d scene");
        // 覆盖仅作用于相机：aspect 同步为传入值，其余相机字段保持覆盖值。
        assert_eq!(overridden.camera.position, Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(overridden.camera.target, Vec3::new(-5.0, -6.0, -7.0));
        assert!((overridden.camera.aspect_ratio - 4.0 / 3.0).abs() < 1e-6);
        assert!((overridden.camera.fov_y - 0.9).abs() < 1e-6);
        // drawables 不受相机覆盖影响：数量与每项名称/mesh path 均一致。
        assert_eq!(overridden.drawables.len(), before.drawables.len());
        for (a, b) in overridden.drawables.iter().zip(before.drawables.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.mesh_path, b.mesh_path);
        }
        // 无覆盖路径 = scene_3d 路径（相机/光源/资源一致），覆盖后场景数据未变。
        let after = runtime.scene_3d(16.0 / 9.0).expect("3d scene");
        assert_eq!(after.camera.position, before.camera.position);
        assert_eq!(after.camera.target, before.camera.target);
        assert_eq!(after.light.direction, before.light.direction);
        assert_eq!(after.drawables.len(), before.drawables.len());
    }

    /// RuntimeScene 无 GPU 长会话：重复 create → run_script_steps → vertices → drop，
    /// 断言无 panic、帧顶点数/纹理路径容器有界（含字体图集反复加载/释放）。
    #[test]
    fn runtime_scene_create_step_drop_100_cycles() {
        let root = game_root();
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let path = root.join("scenes/main.scene");
        assert!(path.is_file(), "测试场景资源缺失: {}", path.display());
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load main: {e}"));
        let input = InputSnapshot::default();
        for cycle in 0..100 {
            let mut runtime =
                RuntimeScene::from_scene_edit(Some(&root), scene.clone()).expect("runtime main");
            for _ in 0..3 {
                runtime
                    .run_script_steps(1, 1.0 / 60.0, &input)
                    .expect("script step");
            }
            let mesh = runtime.vertices(640, 360).expect("vertices");
            assert!(
                mesh.vertices.len() < 1_000_000,
                "cycle {cycle}: 顶点数应有界: {}",
                mesh.vertices.len()
            );
            assert!(
                runtime.texture_paths.len() < 512,
                "cycle {cycle}: 纹理路径容器应有界: {}",
                runtime.texture_paths.len()
            );
            // drop 释放 Lua/世界/字体资源。
        }
    }
}
