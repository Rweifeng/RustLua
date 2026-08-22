//! Runtime scene: instantiates the project scene/scripts and produces
//! per-frame [`SceneMesh`] (2D) and [`Scene3d`] draw data for the renderer.

use std::path::Path;

use engine_core::{
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
    sprite::{Sprite, SpriteAsset, SpriteVertex},
    sprite_batch::{SpriteBatch, SpriteBatchItem},
    text::{TextAtlas, TextLayout, TextMesh, TextStyle},
    tilemap::Tilemap,
    transform::{Transform, Vec2, integrate_velocity},
    transform3d::Transform3D,
};

use crate::frame::{
    ATLAS_TEXTURE_PATH, FONT_ATLAS_TEXTURE_PATH, FontAtlasPixels, OverlayVertex, Scene3d,
    Scene3dDrawable, SceneDraw, SceneMesh, WHITE_TEXTURE_PATH,
};

pub struct RuntimeScene {
    script: LuaRuntime,
    sprite_batch: SpriteBatch,
    camera: Camera2D,
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
        if let Some(root) = project_root {
            script.set_audio_asset_dir(root.join("assets").join("audio"));
            script.set_save_path(root.join(engine_core::PROJECT_WORK_DIR).join("save.sav"));
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
        // 加载 CJK 字体（存在时启用任意字符文本渲染）。
        let font_atlas = project_root
            .map(|root| root.join("assets/fonts/NotoSansSC-VF.ttf"))
            .filter(|path| path.is_file())
            .and_then(|path| {
                let bytes = std::fs::read(&path).ok()?;
                match FontAtlas::load(&bytes) {
                    Ok(atlas) => {
                        tracing::info!(target: "render", path = %path.display(), "[render] CJK font atlas ready");
                        Some(atlas)
                    }
                    Err(error) => {
                        tracing::warn!(target: "render", error = %error, "[render] CJK font atlas load failed; non-ASCII text uses placeholders");
                        None
                    }
                }
            });
        Ok(Self {
            script,
            sprite_batch: SpriteBatch::new(),
            camera,
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
        let drawables = self.script.with_world(|world| {
            world
                .entities_with2::<Transform3D, MeshRenderer>()
                .into_iter()
                .filter_map(|id| {
                    let transform = world.get::<Transform3D>(id).ok().flatten().copied()?;
                    let renderer = world.get::<MeshRenderer>(id).ok().flatten()?;
                    let model = transform.model_matrix().ok()?;
                    // Skinned entities also carry an Animation3D; its joint
                    // palette is recomputed each frame from the active clip.
                    let palette = world
                        .get::<Animation3D>(id)
                        .ok()
                        .flatten()
                        .map(|animation| animation.joint_matrices())
                        .unwrap_or_default();
                    Some(Scene3dDrawable {
                        mesh_path: renderer.mesh.clone(),
                        model,
                        palette,
                    })
                })
                .collect::<Vec<_>>()
        });
        if drawables.is_empty() {
            return None;
        }
        let camera = self.active_camera3d(aspect_ratio);
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
                    Some((transform, sprite, animation, asset))
                })
                .collect::<Vec<_>>()
        });
        // Tile maps draw first (layer 0), sprite entities on top (layer 1+).
        self.push_tiles()?;
        for (index, (transform, sprite, animation, asset)) in sprites.into_iter().enumerate() {
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
            self.sprite_batch.push(SpriteBatchItem::new(
                texture,
                1 + index as i32,
                sprite,
                transform,
            ));
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
            });
        }
        Ok(())
    }

    /// Appends screen-space text glyphs (from the built-in 5x7 atlas) as NDC
    /// quads sampled from the atlas texture.
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
        let atlas = TextAtlas::built_in();
        let mut ascii_vertices = Vec::new();
        let mut cjk_vertices = Vec::new();
        let style = TextStyle {
            scale: 1.0,
            color: [1.0, 1.0, 1.0, 1.0],
            ..Default::default()
        };
        for (position, text, scale) in visible {
            let style = TextStyle { scale, ..style };
            let (ascii_mesh, cjk_mesh) = match self.font_atlas.as_mut() {
                Some(font) => TextLayout::build_mixed(&text, position, style, &atlas, font)
                    .map_err(|error| error.to_string())?,
                None => {
                    let mesh = TextLayout::build(&text, position, style, &atlas)
                        .map_err(|error| error.to_string())?;
                    (mesh, TextMesh::default())
                }
            };
            for vertex in ascii_mesh.vertices {
                ascii_vertices.push(OverlayVertex {
                    position: [
                        vertex.position[0] / width.max(1) as f32 * 2.0 - 1.0,
                        1.0 - vertex.position[1] / height.max(1) as f32 * 2.0,
                    ],
                    uv: vertex.uv,
                    color: vertex.color,
                });
            }
            for vertex in cjk_mesh.vertices {
                cjk_vertices.push(OverlayVertex {
                    position: [
                        vertex.position[0] / width.max(1) as f32 * 2.0 - 1.0,
                        1.0 - vertex.position[1] / height.max(1) as f32 * 2.0,
                    ],
                    uv: vertex.uv,
                    color: vertex.color,
                });
            }
        }
        if !ascii_vertices.is_empty() {
            let start = vertices.len() as u32;
            vertices.extend(ascii_vertices);
            draws.push(SceneDraw {
                texture: self.texture_id(ATLAS_TEXTURE_PATH) + 1,
                start,
                count: vertices.len() as u32 - start,
            });
        }
        if !cjk_vertices.is_empty() {
            let start = vertices.len() as u32;
            vertices.extend(cjk_vertices);
            draws.push(SceneDraw {
                texture: self.texture_id(FONT_ATLAS_TEXTURE_PATH) + 1,
                start,
                count: vertices.len() as u32 - start,
            });
        }
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
                .insert("Sprite".into(), "assets/red-1x1.png".into());
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
    }
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
        },
        OverlayVertex {
            position: [x1, y0],
            uv,
            color,
        },
        OverlayVertex {
            position: [x1, y1],
            uv,
            color,
        },
        OverlayVertex {
            position: [x0, y0],
            uv,
            color,
        },
        OverlayVertex {
            position: [x1, y1],
            uv,
            color,
        },
        OverlayVertex {
            position: [x0, y1],
            uv,
            color,
        },
    ]
}
