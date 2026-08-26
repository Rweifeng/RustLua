//! Runtime scene: instantiates the project scene/scripts and produces
//! per-frame [`SceneMesh`] (2D) and [`Scene3d`] draw data for the renderer.

use std::path::Path;

use crate::core::{
    animation::AnimationStateMachine,
    animation3d::Animation3D,
    camera3d::Camera3D,
    font::FontAtlas,
    input::InputSnapshot,
    light::Light,
    math2d::{Camera2D, Viewport},
    math3d::Vec3,
    mesh3d::MeshRenderer,
    scene::{Scene, SceneEntityId},
    script::LuaRuntime,
    sprite::{Sprite, SpriteAsset, SpriteSorting, SpriteVertex},
    sprite_batch::{SpriteBatch, SpriteBatchItem},
    text::{TextAtlas, TextLayout, TextMesh, TextStyle},
    tilemap::Tilemap,
    transform::{Transform, Vec2, integrate_velocity},
    transform3d::Transform3D,
    ui::{UiButton, UiImage, UiText, resolve_ui_rect, resolve_ui_text_origin},
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
    texture_paths: Vec<String>,
    font_atlas: Option<FontAtlas>,
    last_font_version: u64,
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

    fn init(project_root: Option<&Path>, scene: Scene, run_script: bool) -> Result<Self, String> {
        let mut script = LuaRuntime::new().map_err(|error| error.to_string())?;
        // Project root must be set before instantiation so scene components that
        // load assets (e.g. Animation3D) can resolve their mesh paths.
        if let Some(root) = project_root {
            script.set_project_root(root.to_path_buf());
        }
        let instantiated = script
            .instantiate_scene(&scene)
            .map_err(|error| error.to_string())?;
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
        let camera =
            Camera2D::new(Vec2::new(640.0, 360.0), 1.0).map_err(|error| error.to_string())?;
        tracing::info!(target: "render", "[render] SpriteBatch and Camera2D initialized");
        // 加载字体（存在时启用 SDF 文本渲染）。编辑器场景视图同样需要，这样
        // 场景视图也能显示清晰中文（不再依赖 5x7 位图回退）。
        let font_atlas = project_root
            .map(|root| root.join("assets/fonts/NotoSansSC-VF.ttf"))
            .filter(|path| path.is_file())
            .and_then(|path| {
                let bytes = std::fs::read(&path).ok()?;
                match FontAtlas::load(&bytes) {
                    Ok(atlas) => {
                        tracing::info!(target: "render", path = %path.display(), "[render] font atlas ready");
                        Some(atlas)
                    }
                    Err(error) => {
                        tracing::warn!(target: "render", error = %error, "[render] font atlas load failed; text uses placeholders");
                        None
                    }
                }
            });
        Ok(Self {
            script,
            sprite_batch: SpriteBatch::new(),
            camera,
            viewport_width: 1280,
            viewport_height: 720,
            texture_paths: Vec::new(),
            font_atlas,
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
            // UI Button 点击分发：按下边沿命中矩形 → Lua on_click(name)。
            self.script
                .process_ui_clicks(&step_input, self.viewport_width, self.viewport_height)
                .map_err(|error| error.to_string())?;
            // Physics advances on the fixed timestep and writes dynamic body
            // positions back into ECS Transforms (2D) / Transform3D (3D).
            self.script.step_physics(delta_seconds);
            self.script.step_physics3d(delta_seconds);
            self.script.step_particles(delta_seconds);
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
            for id in world.entities_with::<AnimationStateMachine>() {
                if let Ok(Some(machine)) = world.get_mut::<AnimationStateMachine>(id) {
                    machine.advance(elapsed);
                }
            }
            for id in world.entities_with::<Animation3D>() {
                if let Ok(Some(animation)) = world.get_mut::<Animation3D>(id) {
                    animation.advance(elapsed);
                }
            }
        });
        Ok(())
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
            world
                .entities_with2::<Transform3D, MeshRenderer>()
                .into_iter()
                .filter_map(|id| {
                    let transform = world.get::<Transform3D>(id).ok().flatten().copied()?;
                    let renderer = world.get::<MeshRenderer>(id).ok().flatten()?;
                    let model = transform.model_matrix().ok()?;
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
                            .unwrap_or_else(|_| {
                                crate::core::math3d::Aabb3 {
                                    min: Vec3::new(-1.0, -1.0, -1.0),
                                    max: Vec3::new(1.0, 1.0, 1.0),
                                }
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
                    Some(Scene3dDrawable {
                        entity: SceneEntityId(id.index() as u64),
                        name,
                        mesh_path: renderer.mesh.clone(),
                        model,
                        palette,
                        bounds,
                    })
                })
                .collect::<Vec<_>>()
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
            let candidates = world.entities_with::<Camera3D>();
            let id = selected
                .filter(|id| world.has::<Camera3D>(*id).unwrap_or(false))
                .or_else(|| candidates.first().copied());
            id.and_then(|id| world.get::<Camera3D>(id).ok().flatten().copied())
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
                let candidates = world.entities_with::<Light>();
                let id = selected
                    .filter(|id| world.has::<Light>(*id).unwrap_or(false))
                    .or_else(|| candidates.first().copied());
                id.and_then(|id| world.get::<Light>(id).ok().flatten().copied())
            })
            .unwrap_or_default()
    }

    pub fn vertices(&mut self, width: u32, height: u32) -> Result<SceneMesh, String> {
        self.viewport_width = width.max(1);
        self.viewport_height = height.max(1);
        let viewport = Viewport::new(width.max(1) as f32, height.max(1) as f32)
            .map_err(|error| error.to_string())?;
        self.sprite_batch.clear();
        self.texture_paths.clear();
        let sprites = self.script.with_world(|world| {
            world
                .entities_with::<Sprite>()
                .into_iter()
                .filter_map(|id| {
                    let transform = world.get::<Transform>(id).ok().flatten().copied()?;
                    let sprite = world.get::<Sprite>(id).ok().flatten().copied()?;
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
                    Some((transform, sprite, animation, asset, sorting))
                })
                .collect::<Vec<_>>()
        });
        // Tile maps draw first (layer 0), sprite entities on top (layer 1+).
        self.push_tiles()?;
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
        // Screen-space UI (health bars, prompt text) draws last, on top.
        self.push_ui_bars(&mut vertices, &mut draws, width, height)?;
        self.push_ui_text(&mut vertices, &mut draws, width, height)?;
        // Scene-node UI components (Text / Image / Button) drawn on very top.
        self.push_scene_ui(&mut vertices, &mut draws, width, height)?;
        tracing::info!(
            target: "render",
            draws = draws.len(),
            sdf = draws.iter().filter(|d| d.sdf).count(),
            text_entities = self.script.with_world(|w| w.entities_with::<UiText>().len()),
            "[render] scene ui"
        );
        tracing::debug!(
            target: "render",
            "[render] mesh vertices={} draws={} paths={}",
            vertices.len(),
            draws.len(),
            self.texture_paths.len()
        );
        let font_atlas = if let Some(atlas) = &self.font_atlas {
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
            .map(|text| (text.position, text.text.clone(), text.scale))
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
            world
                .entities_with::<UiImage>()
                .into_iter()
                .filter_map(|id| world.get::<UiImage>(id).ok().flatten().cloned())
                .collect()
        });
        for image in images {
            self.push_image_quad(vertices, draws, width, height, &image)?;
        }
        let buttons: Vec<UiButton> = self.script.with_world(|world| {
            world
                .entities_with::<UiButton>()
                .into_iter()
                .filter_map(|id| world.get::<UiButton>(id).ok().flatten().cloned())
                .collect()
        });
        for button in buttons {
            self.push_button_quad(vertices, draws, width, height, &button)?;
        }
        let texts: Vec<UiText> = self.script.with_world(|world| {
            world
                .entities_with::<UiText>()
                .into_iter()
                .filter_map(|id| world.get::<UiText>(id).ok().flatten().cloned())
                .collect()
        });
        for text in texts {
            let viewport = [width.max(1) as f32, height.max(1) as f32];
            let (_, resolved_scale) = resolve_ui_text_origin(
                text.position,
                [0.0, 0.0],
                text.scale,
                text.space,
                text.anchor,
                viewport,
            );
            let (mesh, is_sdf) =
                self.layout_text(&text.text, [0.0, 0.0], resolved_scale, text.color)?;
            let (origin, _) = resolve_ui_text_origin(
                text.position,
                mesh.size,
                text.scale,
                text.space,
                text.anchor,
                viewport,
            );
            let text_vertices = text_to_ndc_with_offset(mesh, is_sdf, origin, width, height);
            self.push_text_run(vertices, draws, text_vertices, is_sdf);
        }
        Ok(())
    }

    /// Pushes a `Button` by composing a tinted image quad plus centered text.
    fn push_button_quad(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
        button: &UiButton,
    ) -> Result<(), String> {
        let background = UiImage {
            texture: WHITE_TEXTURE_PATH.to_owned(),
            position: button.position,
            size: button.size,
            color: button.background,
            space: button.space,
            anchor: button.anchor,
        };
        self.push_image_quad(vertices, draws, width, height, &background)?;

        if button.label.trim().is_empty() {
            return Ok(());
        }

        let viewport = [width.max(1) as f32, height.max(1) as f32];
        let (origin, size) = resolve_ui_rect(
            button.position,
            button.size,
            button.space,
            button.anchor,
            viewport,
        );
        let label_scale = match button.space {
            crate::core::ui::UiLayoutSpace::Screen => 1.0,
            crate::core::ui::UiLayoutSpace::Canvas => (viewport[0]
                / crate::core::ui::UI_REFERENCE_SIZE[0])
                .min(viewport[1] / crate::core::ui::UI_REFERENCE_SIZE[1]),
        };
        let (mesh, is_sdf) =
            self.layout_text(&button.label, [0.0, 0.0], label_scale, button.text_color)?;
        let dx = origin[0] + (size[0] - mesh.size[0]).max(0.0) / 2.0;
        let dy = origin[1] + (size[1] - mesh.size[1]).max(0.0) / 2.0;
        let text_vertices = text_to_ndc_with_offset(mesh, is_sdf, [dx, dy], width, height);
        self.push_text_run(vertices, draws, text_vertices, is_sdf);
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
        let style = TextStyle {
            scale,
            color: [color[0], color[1], color[2], 1.0],
            ..Default::default()
        };
        if let Some(font) = self.font_atlas.as_mut() {
            TextLayout::build(text, position, style, font)
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
        );
        let x0 = origin[0] / sw * 2.0 - 1.0;
        let y0 = 1.0 - origin[1] / sh * 2.0;
        let x1 = (origin[0] + size[0]) / sw * 2.0 - 1.0;
        let y1 = 1.0 - (origin[1] + size[1]) / sh * 2.0;
        let color = [image.color[0], image.color[1], image.color[2], 1.0];
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
        if let Some(index) = self.texture_paths.iter().position(|entry| entry == path) {
            return index as u32;
        }
        self.texture_paths.push(path.to_owned());
        (self.texture_paths.len() - 1) as u32
    }

    /// Pushes every non-empty tile as a textured quad sampling its tileset
    /// column. All tiles of a map share texture id and layer, so the whole map
    /// is one draw range.
    fn push_tiles(&mut self) -> Result<(), String> {
        let tilemaps = self.script.with_world(|world| {
            world
                .entities_with2::<Transform, Tilemap>()
                .into_iter()
                .filter_map(|id| {
                    let transform = world.get::<Transform>(id).ok().flatten().copied();
                    let tilemap = world.get::<Tilemap>(id).ok().flatten().cloned();
                    transform.zip(tilemap)
                })
                .collect::<Vec<_>>()
        });
        for (transform, tilemap) in tilemaps {
            let texture = self.texture_id(&tilemap.texture);
            for y in 0..tilemap.height {
                for x in 0..tilemap.width {
                    let index = y * tilemap.width + x;
                    let cell = tilemap.tiles[index];
                    if cell == 0 {
                        continue;
                    }
                    let (uv_min, uv_max) = Tilemap::tile_uv(cell);
                    let sprite = Sprite::new(tilemap.tile_size)
                        .map_err(|error| error.to_string())?
                        .with_uv(uv_min, uv_max)
                        .map_err(|error| error.to_string())?;
                    // Center each tile sprite on its cell so visuals align with
                    // the tilemap's per-cell colliders (cell center = x*ts + ts/2).
                    let offset = Vec2::new(
                        (x as f32 + 0.5) * tilemap.tile_size.x * transform.scale.x,
                        (y as f32 + 0.5) * tilemap.tile_size.y * transform.scale.y,
                    );
                    let tile_transform = Transform {
                        position: Vec2::new(
                            transform.position.x + offset.x,
                            transform.position.y + offset.y,
                        ),
                        rotation: transform.rotation,
                        scale: transform.scale,
                    };
                    self.sprite_batch.push(SpriteBatchItem::new(
                        texture,
                        0,
                        sprite,
                        tile_transform,
                    ));
                }
            }
        }
        Ok(())
    }
}

fn load_runtime_scene(project_root: Option<&Path>) -> Scene {
    project_root
        .map(|root| root.join("scenes/main.scene"))
        .and_then(|path| Scene::load(path).ok())
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
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../game_test")
    }

    fn production_sample_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../game_test_2d_production")
    }

    #[test]
    fn ui_test_scene_emits_sdf_text_draws() {
        let root = game_root();
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
        let path = root.join("scenes/menu.scene");
        assert!(path.is_file(), "样板菜单场景缺失: {}", path.display());
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load menu: {e}"));
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime menu");
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        let counts = runtime.script.with_world(|world| {
            (
                world.entities_with::<UiButton>().len(),
                world.entities_with::<UiImage>().len(),
                world.entities_with::<UiText>().len(),
            )
        });
        assert_eq!(counts.0, 2, "menu scene should expose two buttons");
        assert_eq!(counts.1, 1, "menu scene should expose one image");
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
                world.entities_with::<UiButton>().len(),
            )
        });
        assert_eq!(counts.0, 1, "production scene should have one tilemap");
        assert_eq!(
            counts.1, 11,
            "production scene should have player plus ten animated enemies"
        );
        assert_eq!(counts.2, 1, "production scene should have one menu button");
        assert!(
            runtime.texture_paths.len() >= 4,
            "production scene should load tilemap/player/enemy/pickup textures"
        );
        assert!(
            mesh.draws.len() >= 6,
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
