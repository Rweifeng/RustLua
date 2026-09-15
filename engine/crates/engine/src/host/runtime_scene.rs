//! Runtime scene: instantiates the project scene/scripts and produces
//! per-frame [`SceneMesh`] (2D) and [`Scene3d`] draw data for the renderer.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::core::{
    animation::AnimationStateMachine,
    animation3d::Animation3D,
    camera3d::Camera3D,
    entity::EntityId,
    font::{FontAtlas, FontVectorAtlas, project_font},
    input::InputSnapshot,
    light::Light,
    math2d::{Camera2D, Viewport},
    math3d::Vec3,
    mesh3d::MeshRenderer,
    scene::{Scene, SceneEntityId},
    scheduler::{
        ScheduleReport, ScheduleStage, Scheduler, SchedulerError, System, SystemError, SystemTiming,
    },
    script::LuaRuntime,
    sprite::{Sprite, SpriteAsset, SpriteSorting, SpriteVertex},
    sprite_batch::{SpriteBatch, SpriteBatchItem, SpriteBatchMesh, SpriteDrawBatch},
    text::{TextAtlas, TextLayout, TextMesh, TextStyle},
    tilemap::Tilemap,
    transform::{Transform, Vec2, integrate_velocity},
    transform3d::Transform3D,
    ui::{
        DynamicUiImage, UI_REFERENCE_SIZE, UiImage, UiText, nine_slice_cells, resolve_ui_rect,
        ui_text_effective_scale,
    },
    ui_events::{UiClickTarget, UiEvents},
    ui_layout::{UiRect, UiRectLayout},
    ui_scene_layout::{ResolvedUiNode, UiSceneNode, resolve_scene_ui_layout},
};

use crate::host::frame::{
    ATLAS_TEXTURE_PATH, BoundsSource, CustomFontAtlasPixels, FONT_ATLAS_TEXTURE_PATH, FontAtlasMap,
    FontAtlasPixels, FontHandle, OverlayVertex, Scene3d, Scene3dBounds, Scene3dDrawable, SceneDraw,
    SceneMesh, VectorAtlasMap, VectorFontHandle, VectorGlyphSnapshot, VectorSegmentSnapshot,
    VectorTextVertex as FrameVectorTextVertex, WHITE_TEXTURE_PATH, font_texture_path,
};
use crate::host::gamepad::{GamepadBackend, GamepadDeadzone, GamepadPoller};

pub struct RuntimeScene {
    script: LuaRuntime,
    /// 自上次 `take_script_micros` 以来所有固定步里 Lua `update()` 的累计耗时
    /// （微秒）。宿主把它送进 `PerformanceStats::lua_ms`，性能面板因此显示的是
    /// "Lua 花了多少"而不是恒为 0（P4 接线）。
    script_micros: u64,
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
    vector_atlas: Option<Arc<Mutex<FontVectorAtlas>>>,
    /// 场景 Text 自定义字体矢量轮廓缓存（key = `font://<asset>`）。编辑器注入
    /// 同一实例供场景视图/预览跨运行时重建复用；独立运行时用新鲜空表。
    custom_vector_atlases: VectorAtlasMap,
    last_font_version: u64,
    /// 场景 Text 自定义字体缓存（key = `font://<asset>` → 图集句柄）。编辑器注入
    /// 同一实例供场景视图/预览跨运行时重建复用；独立运行时用各自新鲜的空表。
    custom_font_atlases: FontAtlasMap,
    /// 每个自定义字体“上次上报版本”，用于判定本帧像素是否变化（见默认图集的
    /// `last_font_version` 语义，保证新渲染器首帧一定拿到非空像素）。
    custom_font_versions: HashMap<String, u64>,
    /// 本帧布局用到、待上报渲染器的自定义字体 key（`font://...`），`vertices`
    /// 末尾生成像素快照后清空。
    frame_font_keys: Vec<String>,
    /// 已 warn 过的加载失败字体路径（避免文本每帧 layout 时重复刷屏）。
    missing_fonts: HashSet<String>,
    missing_vector_fonts: HashSet<String>,
    text_render_mode: TextRenderMode,
    ui_scene_nodes: BTreeMap<u64, UiSceneNode>,
    ui_layout_rects: BTreeMap<u64, ResolvedUiNode>,
    /// 共享帧调度管线（M18-03）：每个固定步执行一次的系统。调度器实例**跨帧保留**
    /// （只在 `run_script_steps` 期间临时借出再放回），因此系统的拓扑执行序只算一次，
    /// 而不是每步重建调度器 + 重算拓扑序——固定步一帧可跑 8 步，这份开销纯属浪费。
    step_scheduler: Scheduler<RuntimeScene>,
    /// 共享帧调度管线：每次 `run_script_steps` 调用只执行一次的系统。
    frame_scheduler: Scheduler<RuntimeScene>,
    /// 系统注册用**名字**记录，供诊断与重名校验；系统本体归调度器所有。
    step_system_names: Vec<String>,
    frame_system_names: Vec<String>,
    /// 当前固定步的输入快照：由 `run_script_steps` 注入、`input_apply` 消费，
    /// `ui_click_dispatch` 复用，步结束即清空。
    pending_step_input: Option<InputSnapshot>,
    /// 最近一次 `run_script_steps` 的调度统计（M18-03 性能统计出口）。
    last_schedule: Option<FrameScheduleReport>,
    /// 手柄设备轮询（M20-06）：每帧采样后合并进本步输入快照。
    gamepad: GamepadPoller,
}

/// Aggregated scheduler statistics of one `run_script_steps` call (M18-03).
///
/// Per-system `elapsed` values are accumulated across every fixed step of the
/// call, so a system that runs `steps` times reports its total time.
#[derive(Clone, Debug, Default)]
pub struct FrameScheduleReport {
    /// Number of fixed steps the per-step pipeline executed.
    pub steps: u32,
    /// Accumulated wall time of all scheduled pipeline executions.
    pub total_elapsed: Duration,
    /// Per-system timings in first-execution order.
    pub timings: Vec<SystemTiming>,
}

impl FrameScheduleReport {
    fn merge(&mut self, report: &ScheduleReport) {
        for timing in &report.timings {
            if let Some(existing) = self
                .timings
                .iter_mut()
                .find(|entry| entry.name == timing.name)
            {
                existing.elapsed += timing.elapsed;
            } else {
                self.timings.push(timing.clone());
            }
        }
        self.total_elapsed += report.total_elapsed;
    }
}

#[derive(Clone, Copy)]
struct CachedTileSprite {
    center_offset: Vec2,
    uv_min: Vec2,
    uv_max: Vec2,
}

/// 文本渲染后端的项目级策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum TextRenderMode {
    /// 大号短文本用矢量轮廓，其余用 MSDF/位图。
    #[default]
    Auto,
    /// 全部文本优先用矢量轮廓；字体不可用时仍回退 SDF。
    Vector,
    /// 全部文本用 MSDF/内置位图。
    Sdf,
}

impl TextRenderMode {
    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "vector" | "outline" => Self::Vector,
            "sdf" | "msdf" => Self::Sdf,
            _ => Self::Auto,
        }
    }
}

struct CachedTilemapLayer {
    transform: Transform,
    tilemap: Tilemap,
    sprites: Vec<CachedTileSprite>,
}

impl RuntimeScene {
    pub fn new(project_root: Option<&Path>) -> Result<Self, String> {
        let scene = load_runtime_scene(project_root);
        Self::init(project_root, scene, true, false)
    }

    /// P3：显式声明"这是发布包"（`game --pack`）。决定存档与玩家设置写到哪里——
    /// 发布包 → 用户数据目录（`%APPDATA%\<app>`），开发工程 → 工程内。
    /// 不显式声明时 `user_dir::is_release_pack` 仍能凭 `<exe>/pack` + `release.toml`
    /// 认出双击启动的发布包。
    pub fn new_with_layout(project_root: Option<&Path>, packaged: bool) -> Result<Self, String> {
        let scene = load_runtime_scene(project_root);
        Self::init(project_root, scene, true, packaged)
    }

    /// 实例化一个具体场景（编辑器"运行当前场景"用），而非工程的启动场景。
    /// 编辑器处理的永远是开发工程：玩家设置留在工程根（面板直接编辑那份）。
    pub fn from_scene(project_root: Option<&Path>, scene: Scene) -> Result<Self, String> {
        Self::init(project_root, scene, true, false)
    }

    /// Instantiates a scene WITHOUT running the project script, for the editor's
    /// scene view (render the scene as edited, no game logic).
    pub fn from_scene_edit(project_root: Option<&Path>, scene: Scene) -> Result<Self, String> {
        Self::init(project_root, scene, false, false)
    }

    /// Lua 侧 `app.quit()` 只置引擎的退出请求标记（见 `script.rs`），桌面运行器
    /// 借此轮询并在帧末真正退出，否则“退出”按钮在窗口环境无效。
    pub fn quit_requested(&self) -> bool {
        self.script.quit_requested()
    }

    /// 取走并清零"上次取走以来 Lua `update()` 的累计耗时"（微秒）。
    ///
    /// 宿主每次跑完固定步后调用一次，喂给 `PerformanceStats::lua_ms`。用"取走即清零"
    /// 而不是查询，是为了让 0 步帧（帧率高于逻辑步时）自然得到 0，不会把上一帧的
    /// Lua 耗时重复计入。
    pub fn take_script_micros(&mut self) -> u64 {
        std::mem::take(&mut self.script_micros)
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

    /// Updates one authored entity's 2D transform in-place for editor drag
    /// preview. Avoids rebuilding Lua/ECS for every mouse-move revision.
    pub fn set_scene_entity_transform_preview(
        &mut self,
        id: SceneEntityId,
        position: Vec2,
        rotation: f32,
        scale: Vec2,
    ) -> bool {
        let Some(handle) = self.script.scene_entity_handle(id) else {
            return false;
        };
        let mut updated = false;
        self.script.with_world_mut(|world| {
            if let Ok(Some(transform)) = world.get_mut::<Transform>(handle) {
                transform.position = position;
                transform.rotation = rotation;
                transform.scale = scale;
                updated = true;
            }
        });
        updated
    }

    /// Updates one authored UI entity's four-edge offsets in-place for editor
    /// drag preview. The document is only committed when the drag ends.
    pub fn set_scene_entity_ui_layout_preview(
        &mut self,
        id: SceneEntityId,
        offsets: [f32; 4],
    ) -> bool {
        let Some(handle) = self.script.scene_entity_handle(id) else {
            return false;
        };
        let mut updated = false;
        self.script.with_world_mut(|world| {
            if let Ok(Some(layout)) = world.get_mut::<UiRectLayout>(handle) {
                layout.offsets = offsets;
                updated = true;
            }
        });
        if updated {
            // Runtime UI overlay resolves from this snapshot, not the ECS copy.
            // `ui_scene_nodes` is keyed by visible entity order, while `id` is
            // the sparse authoring ID. The live ECS slot index is that order.
            let layout_key = handle.index() as u64;
            if let Some(node) = self.ui_scene_nodes.get_mut(&layout_key)
                && let Some(layout) = node.layout.as_mut()
            {
                layout.offsets = offsets;
            }
            self.ui_layout_rects.clear();
        }
        updated
    }

    /// Returns exact resolved UI rects by authored entity ID. These are the
    /// same rects consumed by the next game-screen render, so editor selection
    /// geometry and rendering share one layout resolution.
    pub fn scene_entity_ui_layout_rects(
        &mut self,
        ids: impl IntoIterator<Item = SceneEntityId>,
        viewport: [f32; 2],
    ) -> BTreeMap<u64, UiRect> {
        self.ensure_scene_ui_layout(viewport);
        ids.into_iter()
            .filter_map(|id| {
                let handle = self.script.scene_entity_handle(id)?;
                self.ui_layout_rects
                    .get(&(handle.index() as u64))
                    .map(|node| (id.0, node.rect))
            })
            .collect()
    }

    /// 覆盖 project.toml 的文本渲染模式（编辑器保存设置后重建运行时可注入）。
    pub fn set_text_render_mode(&mut self, mode: TextRenderMode) {
        self.text_render_mode = mode;
    }

    /// 注入共享字体图集（编辑器在运行时重建时复用，避免每帧重新生成 MSDF 字形）。
    pub fn set_font_atlas(&mut self, font_atlas: Option<Arc<Mutex<FontAtlas>>>) {
        self.font_atlas = font_atlas;
    }

    /// 注入共享默认矢量轮廓图集（编辑器跨运行时重建复用）。
    pub fn set_vector_atlas(&mut self, vector_atlas: Option<VectorFontHandle>) {
        self.vector_atlas = vector_atlas;
    }

    fn ensure_vector_atlas(&mut self) {
        if self.vector_atlas.is_none()
            && let Some(root) = &self.project_root
        {
            self.vector_atlas = load_project_vector_atlas(Some(root));
        }
    }

    /// 获取默认或指定字体的矢量轮廓缓存；自定义字体失败返回 `None`，调用方
    /// 仍可选择回退 SDF。
    fn ensure_vector_font(&mut self, font_asset: &str) -> Option<Arc<Mutex<FontVectorAtlas>>> {
        if font_asset.is_empty() {
            self.ensure_vector_atlas();
            return self.vector_atlas.clone();
        }
        let key = font_texture_path(font_asset);
        if self.missing_vector_fonts.contains(&key) {
            return None;
        }
        if let Ok(map) = self.custom_vector_atlases.lock()
            && let Some(handle) = map.get(&key).cloned()
        {
            return Some(handle);
        }
        if let Some(atlas) = load_custom_vector_atlas(self.project_root.as_deref(), font_asset) {
            if let Ok(mut map) = self.custom_vector_atlases.lock() {
                map.entry(key.clone()).or_insert_with(|| atlas.clone());
            }
            return Some(atlas);
        } else {
            self.missing_vector_fonts.insert(key.clone());
        }
        None
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

    /// 注入跨运行时重建共享的自定义字体缓存（编辑器场景视图/预览用；独立运行
    /// 时运行时各自持有新鲜空表）。
    pub fn set_custom_font_atlases(&mut self, custom_font_atlases: FontAtlasMap) {
        self.custom_font_atlases = custom_font_atlases;
    }

    /// 注入跨运行时重建共享的自定义矢量轮廓缓存。
    pub fn set_custom_vector_atlases(&mut self, custom_vector_atlases: VectorAtlasMap) {
        self.custom_vector_atlases = custom_vector_atlases;
    }

    /// 取场景 Text 指定字体的共享图集句柄；未加载则按需从项目根解码（缺文件/
    /// 不可读返回 None，调用方回退默认字体）。加载失败仅对同一路径 warn 一次。
    fn ensure_custom_font(&mut self, asset: &str) -> Option<FontHandle> {
        let key = font_texture_path(asset);
        if let Ok(map) = self.custom_font_atlases.lock()
            && let Some(handle) = map.get(&key).cloned()
        {
            return Some(handle);
        }
        // 已知缺失：直接回退默认，不再每帧尝试读盘。
        if self.missing_fonts.contains(asset) {
            return None;
        }
        let root = self.project_root.clone();
        let handle = match load_custom_font_atlas(root.as_deref(), asset) {
            Some(handle) => handle,
            None => {
                if self.missing_fonts.insert(asset.to_owned()) {
                    tracing::warn!(
                        target: "render",
                        path = %asset,
                        "[render] custom font load failed; text falls back to default font"
                    );
                }
                return None;
            }
        };
        if let Ok(mut map) = self.custom_font_atlases.lock() {
            let handle = map.entry(key).or_insert_with(|| handle.clone()).clone();
            Some(handle)
        } else {
            None
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

    /// 决定一段文本的首选渲染路径；矢量图集不可用时渲染层仍会回退 SDF。
    fn should_render_vector(&self, font_asset: Option<&str>, scale: f32, chars: usize) -> bool {
        match self.text_render_mode {
            TextRenderMode::Vector => true,
            TextRenderMode::Sdf => false,
            TextRenderMode::Auto => {
                font_asset.is_none_or(str::is_empty) && scale >= 2.0 && chars <= 32
            }
        }
    }

    /// Measures scene-node UI text with the same backend choice as rendering.
    /// `auto` keeps the default-font heuristic; forced modes apply to every text.
    pub fn measure_scene_ui_text(
        &mut self,
        text: &crate::core::ui::UiText,
    ) -> ([f32; 2], [f32; 4]) {
        let scale = text.effective_scale();
        let use_vector =
            self.should_render_vector(Some(text.font.as_str()), scale, text.text.chars().count());
        if use_vector
            && let Some(measured) = self.measure_vector_text(&text.text, scale, &text.font)
        {
            return measured;
        }
        self.measure_ui_text(&text.text, scale)
    }

    fn measure_vector_text(
        &mut self,
        text: &str,
        scale: f32,
        font_asset: &str,
    ) -> Option<([f32; 2], [f32; 4])> {
        let atlas = self.ensure_vector_font(font_asset)?;
        let style = TextStyle {
            scale,
            color: [1.0, 1.0, 1.0, 1.0],
            ..Default::default()
        };
        let mut atlas = atlas.lock().expect("vector atlas lock");
        let layout = TextLayout::build_vector(text, [0.0, 0.0], style, &mut atlas).ok()?;
        if layout.is_empty() {
            return None;
        }
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for vertex in &layout {
            min[0] = min[0].min(vertex.position[0]);
            min[1] = min[1].min(vertex.position[1]);
            max[0] = max[0].max(vertex.position[0]);
            max[1] = max[1].max(vertex.position[1]);
        }
        if !min.iter().all(|value| value.is_finite()) {
            return None;
        }
        Some((
            [max[0] - min[0], max[1] - min[1]],
            [min[0], min[1], max[0], max[1]],
        ))
    }

    fn init(
        project_root: Option<&Path>,
        scene: Scene,
        run_script: bool,
        packaged: bool,
    ) -> Result<Self, String> {
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
            // P3 用户数据目录：发布包（`<exe>/pack` + `release.toml`，或 `--pack`）
            // 把存档与玩家设置写到用户目录（`%APPDATA%\<app>`），不写程序目录；
            // 开发工程保持工程内 `.cache/save.sav` 与工程根 `settings.toml`。
            let roots = crate::core::user_dir::data_roots(root, packaged);
            if let Err(error) = crate::core::user_dir::ensure_save_dir(&roots) {
                tracing::warn!(target: "save", "{}", error);
            }
            if roots.packaged {
                script.set_settings_root(roots.settings_dir.clone());
                match crate::core::user_dir::seed_settings(&roots, root) {
                    Ok(true) => tracing::info!(
                        target: "app",
                        path = %roots.settings_file().display(),
                        "[app] player settings seeded from the release package"
                    ),
                    Ok(false) => {}
                    Err(error) => tracing::warn!(target: "app", "{}", error),
                }
                tracing::info!(
                    target: "save",
                    save = %roots.save_file().display(),
                    settings = %roots.settings_file().display(),
                    app = %roots.app_name,
                    "[save] release layout: user data directory in use"
                );
            }
            script.set_save_path(roots.save_file());
            let script_path = root.join("scripts/main.lua");
            if script_path.is_file() {
                script
                    .execute_project_script(&script_path)
                    .map_err(|error| error.to_string())?;
            } else {
                tracing::info!(target: "script", path = %script_path.display(), "[script] game project has no scripts/main.lua; script runtime idle");
            }
        }
        let text_render_mode = project_text_render_mode(project_root);
        let camera = Camera2D::new(crate::core::math2d::DEFAULT_2D_CAMERA_CENTER, 1.0)
            .map_err(|error| error.to_string())?;
        tracing::info!(target: "render", "[render] SpriteBatch and Camera2D initialized");
        // 字体图集不再在此无条件加载：`load_project_font_atlas` 会按 chars.txt 预热
        // 全部 MSDF 字形，debug 构建下约 2s/项目（实测 ui_demo 135 字 ≈ 2.3s）。
        // 编辑器场景视图/预览通过 `set_font_atlas` 注入共享（已预热）图集；未注入
        // 时由 `ensure_font_atlas` 在首次排版文本时按需加载。这样场景编辑（revision
        // 变化 → 运行时重建）不再每次重建都白白预热一遍被丢弃的图集。
        let ui_scene_nodes = build_ui_scene_nodes(&scene, !run_script);
        let mut scene = Self {
            script,
            script_micros: 0,
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
            vector_atlas: None,
            custom_vector_atlases: Arc::new(Mutex::new(HashMap::new())),
            // 调试插桩：首次 vector text layout 日志状态。
            last_font_version: u64::MAX,
            custom_font_atlases: Arc::new(Mutex::new(HashMap::new())),
            custom_font_versions: HashMap::new(),
            frame_font_keys: Vec::new(),
            missing_fonts: HashSet::new(),
            missing_vector_fonts: HashSet::new(),
            text_render_mode,
            ui_scene_nodes,
            ui_layout_rects: BTreeMap::new(),
            step_scheduler: Scheduler::new(),
            frame_scheduler: Scheduler::new(),
            step_system_names: Vec::new(),
            frame_system_names: Vec::new(),
            pending_step_input: None,
            last_schedule: None,
            gamepad: GamepadPoller::platform_default(),
        };
        scene.register_builtin_frame_systems();
        Ok(scene)
    }

    /// Registers the built-in fixed-stage systems of the runtime frame
    /// pipeline (M18-03).
    ///
    /// The per-step systems preserve the legacy manual orchestration order:
    /// input apply → Lua `update()` → UI click dispatch → 2D/3D physics →
    /// particles → game clock → pending scene transition. The frame systems
    /// run once per `run_script_steps` call: velocity integration (Simulation
    /// stage, after the per-step scene transition) then animation state
    /// machines, matching the legacy post-loop block.
    fn register_builtin_frame_systems(&mut self) {
        self.add_builtin_step_system(System::new(
            "input_apply",
            ScheduleStage::Input,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, _| {
                if let Some(input) = scene.pending_step_input.clone() {
                    scene.script.set_input(input);
                }
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "lua_update",
            ScheduleStage::Script,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, _| {
                let result = scene
                    .script
                    .call_update()
                    .map_err(|error| SystemError::new(error.to_string()))?;
                scene.script_micros = scene
                    .script_micros
                    .saturating_add(scene.script.last_update_micros());
                tracing::debug!(target: "game", "[simulation] fixed update returned: {result}");
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "ui_click_dispatch",
            ScheduleStage::Script,
            ["lua_update"],
            |scene: &mut RuntimeScene, _| {
                let Some(input) = scene.pending_step_input.clone() else {
                    return Err(SystemError::new("pending step input is missing"));
                };
                let targets = scene.compute_click_targets();
                scene
                    .script
                    .process_ui_clicks(&input, &targets)
                    .map_err(|error| SystemError::new(error.to_string()))
            },
        ));
        self.add_builtin_step_system(System::new(
            "physics2d_step",
            ScheduleStage::Physics,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.step_physics(delta_seconds);
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "physics3d_step",
            ScheduleStage::Physics,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.step_physics3d(delta_seconds);
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "particles_step",
            ScheduleStage::Simulation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.step_particles(delta_seconds);
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "clock_step",
            ScheduleStage::Simulation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.step_clock(delta_seconds);
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "scene_transition",
            ScheduleStage::Simulation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, _| {
                // Apply a pending scene.load so level switches happen between
                // steps.
                scene
                    .script
                    .process_scene_transition()
                    .map_err(|error| SystemError::new(error.to_string()))
            },
        ));
        // 以下为每次 run_script_steps 调用只执行一次的帧级系统：原实现里它们
        // 在固定步循环结束后统一推进（elapsed = delta × steps），顺序保持
        // integrate_velocity → AnimationStateMachine → Animation3D。
        self.add_builtin_frame_system(System::new(
            "integrate_velocity",
            ScheduleStage::Simulation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.with_world_mut(|world| {
                    let _ = integrate_velocity(world, delta_seconds);
                });
                Ok(())
            },
        ));
        self.add_builtin_frame_system(System::new(
            "animation_state_advance",
            ScheduleStage::Animation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.with_world_mut(|world| {
                    world.for_each_mut::<AnimationStateMachine>(|_, machine| {
                        machine.advance(delta_seconds)
                    })
                });
                Ok(())
            },
        ));
        self.add_builtin_frame_system(System::new(
            "animation3d_advance",
            ScheduleStage::Animation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.with_world_mut(|world| {
                    world.for_each_mut::<Animation3D>(|_, animation| {
                        animation.advance(delta_seconds)
                    })
                });
                Ok(())
            },
        ));
    }

    /// Advances the runtime by `steps` fixed steps through the shared frame
    /// schedule (M18-03).
    ///
    /// This is the single scheduling entry used by the standalone game shell,
    /// the editor Play preview and headless runners. Every fixed step runs the
    /// per-step pipeline (input → Lua update → UI clicks → physics →
    /// particles → clock → scene transition); after the loop the frame
    /// pipeline runs once (velocity integration → animation advance) with the
    /// accumulated elapsed time, matching the legacy manual orchestration.
    ///
    /// Registered systems are validated by the [`Scheduler`]: missing
    /// dependencies, dependency cycles and system panics surface as errors
    /// naming the offending system and stage. Per-system timings are merged
    /// into [`Self::last_schedule_report`].
    pub fn run_script_steps(
        &mut self,
        steps: u32,
        delta_seconds: f32,
        input: &InputSnapshot,
    ) -> Result<(), String> {
        self.script.set_delta(delta_seconds);
        // M20-06：把真实手柄设备状态合并进本帧输入（只改按钮/轴，不影响键鼠）。
        // 无设备时 XInput 返回未连接，快照与传入值等价，headless 行为不变。
        // M22/W3：轴判定阈值与 `[input] nav_threshold` 同源（Lua `input.set_nav_settings`
        // / `load_settings` 写入 `ScriptState`），每帧同步给设备层，轴的数字边沿因此
        // 与设置文件里的手感一致。
        //
        // M22/W4：**只在真有固定步的帧采样设备**。0 步帧（帧率高于逻辑步，关 vsync 时常见）
        // 里键鼠边沿会被宿主保留到下一有效帧（`game_app.rs` 只在 `steps > 0` 时
        // `finish_frame`），若此时仍 poll 手柄，边沿会写进这个随后被丢弃的 clone 而**永久
        // 丢失**（`poller.last` 已推进，下一帧不会再产生同一变化）。跳过采样后两条路径语义一致。
        let mut step_input = input.clone();
        if steps > 0 {
            self.gamepad
                .set_axis_threshold(self.script.nav_settings().threshold);
            self.gamepad.poll_into(&mut step_input);
        }
        // 调度器实例跨帧保留：借出 → 运行 → 放回。系统本体一直归调度器所有，
        // 因此这里不存在"注册失败丢系统"的路径（此前用 `drain` + `break`，
        // 提前退出会让尚未搬迁的系统随 Drain 一起被丢弃）。
        let mut step_scheduler = std::mem::take(&mut self.step_scheduler);
        let mut frame_scheduler = std::mem::take(&mut self.frame_scheduler);
        let outcome = self.run_pipelines(
            &mut step_scheduler,
            &mut frame_scheduler,
            steps,
            delta_seconds,
            &mut step_input,
        );
        self.step_scheduler = step_scheduler;
        self.frame_scheduler = frame_scheduler;
        // M22/W2：把本帧的震动请求交给设备层（Lua `input.rumble` 只登记请求；到期归零由
        // `GamepadPoller` 的状态机负责）。失败路径不取走请求也没问题——下一帧会覆盖。
        if outcome.is_ok()
            && let Some((low, high, ms)) = self.script.take_rumble_request()
        {
            self.gamepad.rumble(low, high, ms);
        }
        self.pending_step_input = None;
        outcome
    }

    /// Executes the per-step pipeline `steps` times and the frame pipeline
    /// once, accumulating schedule timings into `last_schedule`.
    fn run_pipelines(
        &mut self,
        step_scheduler: &mut Scheduler<RuntimeScene>,
        frame_scheduler: &mut Scheduler<RuntimeScene>,
        steps: u32,
        delta_seconds: f32,
        step_input: &mut InputSnapshot,
    ) -> Result<(), String> {
        let mut report = FrameScheduleReport {
            steps,
            ..FrameScheduleReport::default()
        };
        for step in 0..steps {
            self.pending_step_input = Some(step_input.clone());
            let step_report = step_scheduler
                .run(self, delta_seconds)
                .map_err(|error| error.to_string())?;
            report.merge(&step_report);
            // One-shot edges are consumed by the first fixed step of a frame;
            // clear them so catch-up steps in the same frame don't re-fire
            // just_pressed actions (e.g. F4 switching scenes twice).
            if step == 0 {
                step_input.finish_frame();
            }
        }
        let elapsed = delta_seconds * steps as f32;
        let frame_report = frame_scheduler
            .run(self, elapsed)
            .map_err(|error| error.to_string())?;
        report.merge(&frame_report);
        self.last_schedule = Some(report);
        if tracing::enabled!(tracing::Level::DEBUG)
            && let Some(report) = &self.last_schedule
        {
            let systems = report
                .timings
                .iter()
                .map(|timing| {
                    format!(
                        "{}={:.3}ms",
                        timing.name,
                        timing.elapsed.as_secs_f64() * 1000.0
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            tracing::debug!(target: "scene", "[schedule] steps={} total={:.3}ms systems=[{}]", report.steps, report.total_elapsed.as_secs_f64() * 1000.0, systems);
        }
        Ok(())
    }

    /// Returns the aggregated schedule statistics of the most recent
    /// [`Self::run_script_steps`] call, if any (M18-03 性能统计出口).
    #[must_use]
    pub fn last_schedule_report(&self) -> Option<&FrameScheduleReport> {
        self.last_schedule.as_ref()
    }

    /// 替换手柄后端（M20-06）：headless 验收用 [`FakeBackend`] 脚本化设备事件，
    /// 正式运行时用平台默认（Windows XInput）。
    pub fn set_gamepad_backend(&mut self, backend: Box<dyn GamepadBackend>) {
        self.gamepad = GamepadPoller::new(backend);
    }

    /// 应用玩家配置的摇杆/扳机死区（M20-08，来自 `settings.toml` 的 `[input]` 段）。
    ///
    /// 实现上是**重建平台默认后端**，因此：
    /// * 非 Windows 平台无设备后端，调用为 no-op（静默降级语义不变）；
    /// * 会覆盖此前 `set_gamepad_backend` 注入的后端——宿主启动顺序应是
    ///   `RuntimeScene::new` → `set_gamepad_deadzone`（headless 再注入 Fake）。
    pub fn set_gamepad_deadzone(&mut self, deadzone: GamepadDeadzone) {
        self.gamepad = GamepadPoller::platform_default_with_deadzone(deadzone);
    }

    /// 当前是否检测到手柄设备（M20-06 诊断/UI 可用）。
    #[must_use]
    pub fn gamepad_connected(&self) -> bool {
        self.gamepad.connected()
    }

    /// 手柄后端名称（诊断）。
    #[must_use]
    pub fn gamepad_backend_name(&self) -> &'static str {
        self.gamepad.backend_name()
    }

    /// Grants temporary access to the underlying Lua runtime — the escape
    /// hatch for registered frame systems (and tests) that need to drive
    /// script APIs from a `&mut RuntimeScene` context.
    pub fn with_script<R>(&mut self, visit: impl FnOnce(&mut LuaRuntime) -> R) -> R {
        visit(&mut self.script)
    }

    /// 注册内建 step 系统。名字是静态唯一的；重复只可能是编程错误，而这里的
    /// 失败路径**不静默吞掉**：记 error 日志，并由 `registered_system_names`
    /// 相关测试与调度器自身的重复检测兜住。真正面向调用方的
    /// [`Self::add_step_system`] 会把错误返回给调用者。
    fn add_builtin_step_system(&mut self, system: System<RuntimeScene>) {
        let name = system.name().to_owned();
        if let Err(error) = self.step_scheduler.add_system(system) {
            tracing::error!(
                target: "engine",
                "[runtime] builtin step system `{name}` registration failed: {error}"
            );
            return;
        }
        self.step_system_names.push(name);
    }

    /// 注册内建 frame 系统（语义同 [`Self::add_builtin_step_system`]）。
    fn add_builtin_frame_system(&mut self, system: System<RuntimeScene>) {
        let name = system.name().to_owned();
        if let Err(error) = self.frame_scheduler.add_system(system) {
            tracing::error!(
                target: "engine",
                "[runtime] builtin frame system `{name}` registration failed: {error}"
            );
            return;
        }
        self.frame_system_names.push(name);
    }

    /// Registers an extra system that runs on every fixed step of
    /// [`Self::run_script_steps`] (M18-03).
    ///
    /// The system joins the shared schedule: it declares a
    /// [`ScheduleStage`] and dependencies on other registered systems, and
    /// the scheduler validates the resulting order (cached until the
    /// registration changes). Duplicate names are rejected here; missing
    /// dependencies and cycles surface as locatable errors from
    /// [`Self::run_script_steps`].
    pub fn add_step_system(&mut self, system: System<RuntimeScene>) -> Result<(), SchedulerError> {
        let name = system.name().to_owned();
        self.ensure_unique_system_name(&name)?;
        self.step_scheduler.add_system(system)?;
        self.step_system_names.push(name);
        Ok(())
    }

    /// Registers an extra system that runs once per
    /// [`Self::run_script_steps`] call (M18-03). See [`Self::add_step_system`].
    pub fn add_frame_system(&mut self, system: System<RuntimeScene>) -> Result<(), SchedulerError> {
        let name = system.name().to_owned();
        self.ensure_unique_system_name(&name)?;
        self.frame_scheduler.add_system(system)?;
        self.frame_system_names.push(name);
        Ok(())
    }

    /// Returns the registered per-step and frame system names in execution
    /// (registration) order, for diagnostics and tests.
    #[must_use]
    pub fn registered_system_names(&self) -> (Vec<String>, Vec<String>) {
        (
            self.step_system_names.clone(),
            self.frame_system_names.clone(),
        )
    }

    fn ensure_unique_system_name(&self, name: &str) -> Result<(), SchedulerError> {
        if self
            .step_system_names
            .iter()
            .chain(self.frame_system_names.iter())
            .any(|registered| registered == name)
        {
            return Err(SchedulerError::DuplicateSystem {
                name: name.to_owned(),
            });
        }
        Ok(())
    }

    /// 计算当前所有 `UiEvents` 实体的命中目标（屏幕像素）。
    ///
    /// Image/Text 用 UI 解析（viewport=实际渲染尺寸、reference=设计分辨率），
    /// Sprite 用相机投影到屏幕；供 `process_ui_clicks` 点选分发。
    fn compute_click_targets(&mut self) -> Vec<UiClickTarget> {
        let viewport = Viewport {
            width: self.viewport_width.max(1) as f32,
            height: self.viewport_height.max(1) as f32,
        };
        let viewport_arr = [viewport.width, viewport.height];
        self.ensure_scene_ui_layout(viewport_arr);
        // 先提取 UiEvents 实体及其视觉组件（world 借用结束后再计算，避免借用冲突）。
        type UiEventComponents = (
            EntityId,
            Option<UiImage>,
            Option<UiText>,
            Option<(Sprite, Transform)>,
            Option<crate::core::ui_events::UiEvents>,
        );
        let entities: Vec<UiEventComponents> = self.script.with_world(|world| {
            let mut out = Vec::new();
            for id in world.entities_with::<crate::core::ui_events::UiEvents>() {
                let image = world.get::<UiImage>(id).ok().flatten().cloned();
                let text = world.get::<UiText>(id).ok().flatten().cloned();
                let sprite = world.get::<Sprite>(id).ok().flatten().cloned();
                let transform = world.get::<Transform>(id).ok().flatten().cloned();
                let events = world
                    .get::<crate::core::ui_events::UiEvents>(id)
                    .ok()
                    .flatten()
                    .cloned();
                out.push((id, image, text, sprite.zip(transform), events));
            }
            out
        });
        let camera = self.camera;
        entities
            .into_iter()
            .filter_map(|(id, image, text, sprite, events)| {
                let name = self.script.entity_name(id)?;
                let rect: ([f32; 2], [f32; 2]) = if image.is_some() {
                    let layout_rect = self.scene_ui_layout_rect(id, viewport_arr)?;
                    (
                        [layout_rect.x, layout_rect.y],
                        [layout_rect.w, layout_rect.h],
                    )
                } else if let Some(text) = text {
                    let layout_rect = self.scene_ui_layout_rect(id, viewport_arr)?;
                    self.text_hit_rect(&layout_rect, &text)?
                } else if let Some((sprite, transform)) = sprite {
                    sprite_screen_rect(&sprite, &transform, camera, viewport)?
                } else {
                    return None;
                };
                let functions =
                    events.map_or_else(Vec::new, |events| UiEvents::click_functions(&events));
                Some(UiClickTarget {
                    name,
                    origin: rect.0,
                    size: rect.1,
                    functions,
                })
            })
            .collect()
    }

    /// 文本 hit 矩形：与编辑器 UI 包围框同款——resolve 出网格原点，度量字形
    /// bbox，返回 `(glyph_origin, glyph_size)`（屏幕像素）。
    fn text_hit_rect(
        &mut self,
        layout_rect: &UiRect,
        text: &UiText,
    ) -> Option<([f32; 2], [f32; 2])> {
        let (_, glyph_bounds) = self.measure_scene_ui_text(text);
        let origin = [layout_rect.x, layout_rect.y];
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
        // 每帧先清空：布局阶段（push_scene_ui 等）再把本帧用到的自定义字体 key 记入。
        self.frame_font_keys.clear();
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
        let mut vector_vertices = Vec::with_capacity(256);
        let mut vector_glyphs = Vec::with_capacity(64);
        let mut vector_segments = Vec::with_capacity(1024);
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
                vector: false,
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
            self.push_ui_text(
                &mut vertices,
                &mut draws,
                &mut vector_vertices,
                &mut vector_glyphs,
                &mut vector_segments,
                width,
                height,
            )?;
            // Scene-node UI components (Text / Image / Button) drawn on very top.
            self.push_scene_ui(
                &mut vertices,
                &mut draws,
                &mut vector_vertices,
                &mut vector_glyphs,
                &mut vector_segments,
                width,
                height,
            )?;
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
            // 仅当本帧网格会被渲染器实际消费（vertices 非空）时才推进“已上报”
            // 版本号。空网格帧渲染器整体跳过字体上传；若此刻推进版本号，下一个
            // 可绘制帧会以“已上报”发送空像素，而渲染器从未上传过该版本 →
            // write_texture 以空数据写整张图集 → wgpu 校验崩溃。
            let changed = version != self.last_font_version && !vertices.is_empty();
            if !vertices.is_empty() {
                self.last_font_version = version;
            }
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
        let custom_font_atlases = self.snapshot_custom_fonts(!vertices.is_empty());
        Ok(SceneMesh {
            vertices,
            draws,
            paths: self.texture_paths.clone(),
            font_atlas,
            custom_font_atlases,
            vector_vertices,
            vector_glyphs,
            vector_segments,
        })
    }

    /// 生成本帧用到（且自上次上报后版本变化）的自定义字体图集像素，随 SceneMesh
    /// 交给渲染器上传/重绑。`pixels` 空 = 未变化（与默认图集 `font_atlas` 语义一致）。
    ///
    /// `will_draw` 必须与默认图集用同一个判据（本帧 `vertices` 非空）：渲染器在
    /// `vertices` 为空时**整体跳过** 2D 通道，自定义字体的上传与 bind 建立也随之
    /// 跳过。若此时仍把版本记为"已上报"，下一个可绘制帧会以"未变化"（空像素）
    /// 发送，而渲染器从未上传过该版本 → 该自定义字体永久不显示。
    fn snapshot_custom_fonts(&mut self, will_draw: bool) -> Vec<CustomFontAtlasPixels> {
        let mut seen: HashSet<String> = HashSet::new();
        let keys: Vec<String> = std::mem::take(&mut self.frame_font_keys)
            .into_iter()
            .filter(|key| seen.insert(key.clone()))
            .collect();
        let mut out = Vec::new();
        for key in keys {
            let handle = match self.custom_font_atlases.lock() {
                Ok(map) => map.get(&key).cloned(),
                Err(_) => None,
            };
            let Some(handle) = handle else {
                continue;
            };
            let atlas = handle.lock().expect("font atlas lock");
            let version = atlas.version();
            let changed = self.custom_font_versions.get(&key).copied() != Some(version);
            if will_draw {
                self.custom_font_versions.insert(key.clone(), version);
            }
            out.push(CustomFontAtlasPixels {
                key,
                pixels: FontAtlasPixels {
                    width: atlas.width(),
                    height: atlas.height(),
                    // 未被绘制的帧不复制像素：既不浪费内存，也不推进"已上报"版本。
                    pixels: if changed && will_draw {
                        atlas.pixels().to_vec()
                    } else {
                        Vec::new()
                    },
                    version,
                },
            });
        }
        out
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

    /// E16：实体带 `UiLayout` 组件时，其四边 Anchor+Offset 布局优先于
    /// Text/Image 的内联位置数据。把解析矩形回写为 Screen/[0,0] 内联值即可
    /// 复用既有渲染路径——视觉属性（纹理/颜色/文本）仍来自原组件。
    fn scene_ui_layout_rect(&self, id: EntityId, _viewport: [f32; 2]) -> Option<UiRect> {
        self.ui_layout_rects
            .get(&(id.index() as u64))
            .map(|node| node.rect)
    }

    fn ensure_scene_ui_layout(&mut self, viewport: [f32; 2]) {
        let (mut resolved, _) = resolve_scene_ui_layout(&self.ui_scene_nodes, viewport);
        // The resolver also gives non-layout roots a preferred-size rect at the
        // origin. That default is for hierarchy traversal; only authored layouts
        // or container placements may override it.
        resolved.retain(|id, resolved| {
            resolved.container_controlled
                || self
                    .ui_scene_nodes
                    .get(id)
                    .is_some_and(|node| node.layout.is_some())
        });
        self.ui_layout_rects = resolved;
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
                vector: false,
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
                vector: false,
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
                    vector: false,
                });
                if let Some(texture) = cell.texture {
                    let image = DynamicUiImage {
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
                vector: false,
            });
        }
        Ok(())
    }

    /// Appends screen-space text glyphs (SDF font atlas) as NDC quads. Shares
    /// `layout_text` with the scene-node UI Text component.
    #[allow(clippy::too_many_arguments)]
    fn push_ui_text(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        vector_vertices: &mut Vec<FrameVectorTextVertex>,
        vector_glyphs: &mut Vec<VectorGlyphSnapshot>,
        vector_segments: &mut Vec<VectorSegmentSnapshot>,
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
                    text.font_size.map_or(text.scale, ui_text_effective_scale),
                )
            })
            .collect();
        if visible.is_empty() {
            return Ok(());
        }
        for (position, text, scale) in visible {
            let use_vector = self.should_render_vector(None, scale, text.chars().count());
            if !use_vector
                || !self.push_vector_text_run(
                    vector_vertices,
                    vector_glyphs,
                    vector_segments,
                    &text,
                    position,
                    scale,
                    [1.0, 1.0, 1.0],
                    None,
                    width,
                    height,
                )?
            {
                let (mesh, is_sdf) = self.layout_text(&text, position, scale, [1.0, 1.0, 1.0])?;
                let text_vertices = text_to_ndc(mesh, is_sdf, width, height);
                let font_key = if is_sdf {
                    FONT_ATLAS_TEXTURE_PATH.to_owned()
                } else {
                    ATLAS_TEXTURE_PATH.to_owned()
                };
                self.push_text_run(vertices, draws, text_vertices, is_sdf, &font_key);
            }
        }
        Ok(())
    }

    /// Pushes scene-node UI components (`Text` / `Image` / `Button`) as NDC
    /// overlay quads on top of the Lua UI. Order: images, buttons, then text.
    /// Buttons render by composing the same image/text helpers used elsewhere.
    #[allow(clippy::too_many_arguments)]
    fn push_scene_ui(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        vector_vertices: &mut Vec<FrameVectorTextVertex>,
        vector_glyphs: &mut Vec<VectorGlyphSnapshot>,
        vector_segments: &mut Vec<VectorSegmentSnapshot>,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let viewport = [width.max(1) as f32, height.max(1) as f32];
        self.ensure_scene_ui_layout(viewport);
        let images: Vec<(EntityId, UiImage)> = self.script.with_world(|world| {
            let mut images = Vec::new();
            world.for_each::<UiImage>(|id, image| images.push((id, image.clone())));
            images
        });
        for (id, image) in images {
            if let Some(rect) = self.scene_ui_layout_rect(id, viewport) {
                let image = DynamicUiImage {
                    texture: image.texture,
                    position: [rect.x, rect.y],
                    size: [rect.w, rect.h],
                    color: image.color,
                    space: crate::core::ui::UiLayoutSpace::Screen,
                    anchor: [0.0, 0.0],
                    nine_slice: image.nine_slice,
                };
                self.push_image_quad(vertices, draws, width, height, &image)?;
            }
        }
        let texts: Vec<(EntityId, UiText)> = self.script.with_world(|world| {
            let mut texts = Vec::new();
            world.for_each::<UiText>(|id, text| texts.push((id, text.clone())));
            texts
        });
        // 关键：先把本帧全部文本的字形预热进图集，再逐条构建网格。若在排版过程
        // 中才光栅化，图集会增长（1024→2048…）；`FontAtlas::grow` 只重缩放已缓存
        // 字形的归一化 UV，不会修正已经写进顶点缓冲的 UV，于是同一帧内先构建的
        // 网格会用旧尺寸的 UV 在更大的图集里采样到 2 倍坐标处的错误字形，表现为
        // 白块 / 缺笔画。预热后图集尺寸在本帧保持稳定。
        self.prewarm_frame_glyphs(&texts);
        for (id, text) in texts {
            let rect = self.scene_ui_layout_rect(id, viewport);
            if let Some(rect) = rect {
                let position = [rect.x, rect.y];
                // `layout_text` already emits absolute screen-space vertices;
                // adding the layout origin again would displace scene text by
                // its full top-left position (the visible "text ran away" bug).
                let use_vector = self.should_render_vector(
                    Some(text.font.as_str()),
                    text.effective_scale(),
                    text.text.chars().count(),
                );
                if use_vector
                    && self.push_vector_text_run(
                        vector_vertices,
                        vector_glyphs,
                        vector_segments,
                        &text.text,
                        position,
                        text.effective_scale(),
                        text.color,
                        Some(&text.font),
                        width,
                        height,
                    )?
                {
                    continue;
                }
                let (mesh, is_sdf, font_key) = self.layout_scene_text(&text, position)?;
                let text_vertices = text_to_ndc(mesh, is_sdf, width, height);
                self.push_text_run(vertices, draws, text_vertices, is_sdf, &font_key);
            }
        }
        Ok(())
    }

    /// Layouts a string into a pixel-space text mesh using real font metrics.
    /// Uses the SDF font atlas when available, else the built-in bitmap atlas.
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

    #[allow(clippy::too_many_arguments)]
    fn push_vector_text_run(
        &mut self,
        vertices: &mut Vec<FrameVectorTextVertex>,
        glyphs: &mut Vec<VectorGlyphSnapshot>,
        segments: &mut Vec<VectorSegmentSnapshot>,
        text: &str,
        position: [f32; 2],
        scale: f32,
        color: [f32; 3],
        font_asset: Option<&str>,
        width: u32,
        height: u32,
    ) -> Result<bool, String> {
        let atlas = self.ensure_vector_font(font_asset.unwrap_or_default());
        let Some(atlas) = atlas else {
            return Ok(false);
        };
        let style = TextStyle {
            scale,
            color: [color[0], color[1], color[2], 1.0],
            ..Default::default()
        };
        let mut atlas = atlas.lock().expect("vector atlas lock");
        let layout = TextLayout::build_vector(text, position, style, &mut atlas)
            .map_err(|error| error.to_string())?;
        if layout.is_empty() {
            return Ok(false);
        }
        let base_glyph = glyphs.len() as u32;
        let mut remap = HashMap::new();
        for vertex in layout {
            let mapped = if let Some(mapped) = remap.get(&vertex.glyph_id) {
                *mapped
            } else {
                let Some(glyph) = atlas.glyph_by_id(vertex.glyph_id) else {
                    return Ok(false);
                };
                let mapped = base_glyph + remap.len() as u32;
                let segment_start = segments.len() as u32;
                segments.extend(glyph.segments.iter().map(|segment| VectorSegmentSnapshot {
                    start: segment.start,
                    end: segment.end,
                    bounds: [
                        segment.start[0].min(segment.end[0]),
                        segment.start[1].min(segment.end[1]),
                        segment.start[0].max(segment.end[0]),
                        segment.start[1].max(segment.end[1]),
                    ],
                }));
                glyphs.push(VectorGlyphSnapshot {
                    segment_start,
                    segment_count: segments.len() as u32 - segment_start,
                    _padding: [0; 2],
                    bounds: glyph.bounds,
                });
                remap.insert(vertex.glyph_id, mapped);
                mapped
            };
            vertices.push(FrameVectorTextVertex {
                position: [
                    vertex.position[0] / width.max(1) as f32 * 2.0 - 1.0,
                    1.0 - vertex.position[1] / height.max(1) as f32 * 2.0,
                ],
                local: vertex.local,
                glyph_id: mapped,
                color: vertex.color,
            });
        }
        Ok(true)
    }

    /// 在构建任何文本网格之前，把本帧全部文本所需字形光栅化进各自字体图集，
    /// 使图集在本帧增长到最终尺寸。见 `push_scene_ui` 调用处注释。
    fn prewarm_frame_glyphs(&mut self, texts: &[(EntityId, UiText)]) {
        let mut default_chars: Vec<char> = Vec::new();
        let mut custom_chars: HashMap<&str, Vec<char>> = HashMap::new();
        for (_, text) in texts {
            if self.should_render_vector(
                Some(text.font.as_str()),
                text.effective_scale(),
                text.text.chars().count(),
            ) {
                continue;
            }
            if text.font.is_empty() {
                default_chars.extend(text.text.chars());
            } else {
                custom_chars
                    .entry(text.font.as_str())
                    .or_default()
                    .extend(text.text.chars());
            }
        }
        if !default_chars.is_empty() {
            self.ensure_font_atlas();
            if let Some(font) = &self.font_atlas {
                let mut font = font.lock().expect("font atlas lock");
                font.prewarm(default_chars.into_iter());
            }
        }
        for (asset, chars) in custom_chars {
            if let Some(handle) = self.ensure_custom_font(asset) {
                let mut atlas = handle.lock().expect("font atlas lock");
                atlas.prewarm(chars.into_iter());
            }
        }
    }

    /// 记录入本帧待上报列表；无自定义字体 / 字体缺失不可读时回退默认（默认 SDF 或
    /// 内置位图）。返回 `(mesh, is_sdf, font_key)`。
    fn layout_scene_text(
        &mut self,
        text: &UiText,
        position: [f32; 2],
    ) -> Result<(TextMesh, bool, String), String> {
        let effective_scale = text.effective_scale();
        if !text.font.is_empty()
            && let Some(handle) = self.ensure_custom_font(&text.font)
        {
            let mesh = {
                let mut atlas = handle.lock().expect("font atlas lock");
                let style = TextStyle {
                    scale: effective_scale,
                    color: [text.color[0], text.color[1], text.color[2], 1.0],
                    ..Default::default()
                };
                TextLayout::build(&text.text, position, style, &mut atlas)
                    .map_err(|error| error.to_string())?
            };
            let key = font_texture_path(&text.font);
            self.frame_font_keys.push(key.clone());
            return Ok((mesh, true, key));
        }
        // 无自定义字体，或自定义字体缺失/不可读：回退默认字体路径。
        let (mesh, is_sdf) = self.layout_text(&text.text, position, effective_scale, text.color)?;
        let key = if is_sdf {
            FONT_ATLAS_TEXTURE_PATH.to_owned()
        } else {
            ATLAS_TEXTURE_PATH.to_owned()
        };
        Ok((mesh, is_sdf, key))
    }

    /// Appends a text run as a single SDF or bitmap draw. SDF runs carry their
    /// font key (`font://...` for a custom font, else the default-atlas sentinel);
    /// bitmap runs always bind the built-in 5x7 atlas.
    fn push_text_run(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        text_vertices: Vec<OverlayVertex>,
        is_sdf: bool,
        font_key: &str,
    ) {
        if text_vertices.is_empty() {
            return;
        }
        let start = vertices.len() as u32;
        let texture = if is_sdf {
            self.texture_id(font_key) + 1
        } else {
            self.texture_id(ATLAS_TEXTURE_PATH) + 1
        };
        vertices.extend(text_vertices);
        draws.push(SceneDraw {
            texture,
            start,
            count: vertices.len() as u32 - start,
            sdf: is_sdf,
            vector: false,
        });
    }

    /// Pushes a screen-space textured quad for the `Image` component.
    fn push_image_quad(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
        image: &DynamicUiImage,
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
            && let Some((tw, th)) = self.texture_dimensions(&image.texture)
        {
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
                    vector: false,
                });
            }
            return Ok(());
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
            vector: false,
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

/// Captures the serialized scene hierarchy and UI layout data once. Runtime
/// entity slots are allocated in scene iteration order, so layout nodes are
/// keyed by that order rather than by the scene file's sparse entity ids.
fn build_ui_scene_nodes(scene: &Scene, honor_editor_hidden: bool) -> BTreeMap<u64, UiSceneNode> {
    let mut id_to_key = BTreeMap::new();
    for entity in scene.entities.values() {
        if honor_editor_hidden && scene.is_entity_hidden_with_ancestors(entity.id) {
            continue;
        }
        id_to_key.insert(entity.id.0, id_to_key.len() as u64);
    }
    let mut nodes = BTreeMap::new();
    for entity in scene.entities.values() {
        if honor_editor_hidden && scene.is_entity_hidden_with_ancestors(entity.id) {
            continue;
        }
        let Some(&entity_key) = id_to_key.get(&entity.id.0) else {
            continue;
        };
        let layout = entity
            .components
            .get("UiLayout")
            .and_then(|raw| UiRectLayout::from_component_str(raw).ok());
        let container = entity
            .components
            .get("Container")
            .and_then(|raw| crate::core::ui_layout::ContainerStyle::from_component_str(raw).ok());
        let preferred_size = if entity.components.contains_key("Image") {
            layout
                .as_ref()
                .map(|layout| layout.resolve([0.0, 0.0]))
                .map_or([64.0, 64.0], |rect| [rect.w, rect.h])
        } else {
            [64.0, 64.0]
        };
        nodes.insert(
            entity_key,
            UiSceneNode {
                parent: entity.parent.and_then(|id| id_to_key.get(&id.0)).copied(),
                layout,
                container,
                preferred_size,
            },
        );
    }
    nodes
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
    project_font::read_key(project_root, "startup_scene")
        .unwrap_or_else(|| "scenes/main.scene".to_owned())
}

/// project.toml 的 `text_render_mode` 键；无效/缺省回退 `auto`。
fn project_text_render_mode(project_root: Option<&Path>) -> TextRenderMode {
    project_root
        .and_then(|root| project_font::read_key(root, "text_render_mode"))
        .map(|mode| TextRenderMode::parse(&mode))
        .unwrap_or(TextRenderMode::Auto)
}

/// 加载项目字体并包装为可跨运行时共享的矢量轮廓缓存。
pub fn load_project_vector_atlas(
    project_root: Option<&Path>,
) -> Option<Arc<Mutex<FontVectorAtlas>>> {
    let root = project_root?;
    let primary = fs::read(root.join(project_font::primary_path(root))).ok()?;
    let primary = crate::core::pack::decode(&primary).ok()?;
    let fallback = project_font::fallback_path(root).and_then(|path| {
        let bytes = fs::read(root.join(path)).ok()?;
        crate::core::pack::decode(&bytes).ok()
    });
    FontVectorAtlas::load_with_fallback(&primary, fallback.as_deref())
        .ok()
        .map(|atlas| Arc::new(Mutex::new(atlas)))
}

/// 加载项目字体并包装为可跨运行时共享的图集（MSDF 字形按需生成，共享后只生成
/// 缺省路径是 NotoSansSC-VF.ttf，项目可通过 `font` 键切换字体；
/// 缺省字重是 Regular（400）。
pub fn load_project_font_atlas(project_root: Option<&Path>) -> Option<Arc<Mutex<FontAtlas>>> {
    let root = project_root?;
    let primary_path = {
        let path = root.join(project_font::primary_path(root));
        path.is_file().then_some(path)
    }?;
    let primary_bytes = fs::read(&primary_path).ok()?;
    let primary_bytes = crate::core::pack::decode(&primary_bytes).ok()?;
    // 可选后备字体（project.toml `font_fallback` 键），用于主字体缺失的字形。
    let fallback_bytes = project_font::fallback_path(root)
        .and_then(|font| {
            let path = root.join(font);
            path.is_file().then_some(path)
        })
        .and_then(|path| {
            let bytes = fs::read(&path).ok()?;
            crate::core::pack::decode(&bytes).ok()
        });
    let font_weight = project_font::weight(root);
    let mut atlas = match FontAtlas::load_with_fallback_and_weight(
        &primary_bytes,
        fallback_bytes.as_deref(),
        font_weight,
    ) {
        Ok(atlas) => atlas,
        Err(error) => {
            tracing::warn!(target: "render", error = %error, "[render] font atlas load failed; text uses placeholders");
            return None;
        }
    };
    tracing::info!(
        target: "render",
        path = %primary_path.display(),
        weight = font_weight,
        "[render] font atlas ready"
    );
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

pub fn load_custom_vector_atlas(
    project_root: Option<&Path>,
    font_asset: &str,
) -> Option<Arc<Mutex<FontVectorAtlas>>> {
    let root = project_root?;
    if font_asset.trim().is_empty() {
        return None;
    }
    let bytes = fs::read(root.join(font_asset)).ok()?;
    let bytes = crate::core::pack::decode(&bytes).ok()?;
    let fallback = project_font::fallback_path(root).and_then(|path| {
        let bytes = fs::read(root.join(path)).ok()?;
        crate::core::pack::decode(&bytes).ok()
    });
    FontVectorAtlas::load_with_fallback(&bytes, fallback.as_deref())
        .ok()
        .map(|atlas| Arc::new(Mutex::new(atlas)))
}

/// 读取 + pack 解码 + 项目 fallback 字体 + 项目字重。**不做** chars.txt 预热
/// （字形按需光栅化）。任何失败返回 `None` → 调用方回退默认字体，不中断渲染
/// （wasm / 发布包缺该字体时自动降级）。
pub fn load_custom_font_atlas(project_root: Option<&Path>, font_asset: &str) -> Option<FontHandle> {
    let root = project_root?;
    if font_asset.trim().is_empty() {
        return None;
    }
    let path = root.join(font_asset);
    if !path.is_file() {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    let bytes = crate::core::pack::decode(&bytes).ok()?;
    // 与默认字体一致地附上项目后备字体，缺字时回退（如拉丁字体不含 CJK）。
    let fallback_bytes = project_font::fallback_path(root)
        .and_then(|font| {
            let path = root.join(font);
            path.is_file().then_some(path)
        })
        .and_then(|path| {
            let bytes = fs::read(&path).ok()?;
            crate::core::pack::decode(&bytes).ok()
        });
    let font_weight = project_font::weight(root);
    let atlas = match FontAtlas::load_with_fallback_and_weight(
        &bytes,
        fallback_bytes.as_deref(),
        font_weight,
    ) {
        Ok(atlas) => atlas,
        Err(error) => {
            tracing::warn!(
                target: "render",
                error = %error,
                path = %font_asset,
                "[render] custom font atlas load failed; text falls back to default"
            );
            return None;
        }
    };
    tracing::info!(target: "render", path = %font_asset, "[render] custom font atlas ready");
    Some(Arc::new(Mutex::new(atlas)))
}

/// Sprite 的世界矩形（pivot+缩放+旋转+位移）投影到屏幕的 AABB，供 UiEvents 命中。
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
            && last.start + last.count == draw.start
            && last.sdf == draw.sdf
            && last.vector == draw.vector
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

    // 示例重组后，展示工程从 examples/ 迁入 examples/game_api/（多项目布局）；
    // 此处路径同步修正，恢复 showcase/ui_test 等测试的真实覆盖（M18-03）。
    fn game_root() -> std::path::PathBuf {
        crate::core::games_dir().join("examples").join("game_api")
    }

    #[test]
    fn showcase_scenes_instantiate_and_step() {
        let root = game_root();
        let names = [
            "arena",
            "data_inventory",
            "demo3d",
            "effects",
            "game3d",
            "gameplay_api",
            "index",
            "main",
            "menu",
            "persistence",
            "physics3d",
            "production",
            "resources",
            "stress",
            "tilemap",
            "ui_dynamic",
            "ui_layout",
            "ui_test",
        ];
        for name in names {
            let path = root.join("scenes").join(format!("{name}.scene"));
            let scene = Scene::load(&path).unwrap_or_else(|error| panic!("load {name}: {error}"));
            let mut runtime = RuntimeScene::from_scene(Some(&root), scene)
                .unwrap_or_else(|error| panic!("instantiate {name}: {error}"));
            runtime
                .run_script_steps(1, 1.0 / 60.0, &crate::core::input::InputSnapshot::default())
                .unwrap_or_else(|error| panic!("step {name}: {error}"));
        }
    }

    fn production_sample_root() -> std::path::PathBuf {
        crate::core::games_dir().join("examples").join("game_api")
    }

    #[test]
    fn example_project_runs_with_new_api() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        let mut runtime = RuntimeScene::new(Some(&root)).unwrap();
        runtime
            .run_script_steps(2, 1.0 / 60.0, &crate::core::input::InputSnapshot::default())
            .unwrap();
    }

    /// P4：`lua_ms` 指标真的被填了。此前 `begin_lua/end_lua` 只有单测在调，
    /// 真实运行的 `[perf] lua_ms=` 恒为 `0.000`——一条看着在工作、其实永远为零的指标。
    #[test]
    fn script_update_time_is_measured_and_taken_once() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        let mut runtime = RuntimeScene::new(Some(&root)).unwrap();
        runtime
            .run_script_steps(2, 1.0 / 60.0, &crate::core::input::InputSnapshot::default())
            .unwrap();
        let micros = runtime.take_script_micros();
        assert!(micros > 0, "Lua update 耗时必须被测量到，得到 {micros}µs");
        // 取走即清零：0 步帧不该重复计入上一帧的耗时。
        assert_eq!(runtime.take_script_micros(), 0);
    }

    /// ui_test 示例场景：渲染出 UI/世界 draw、UiEvents 目标存在，且点击
    /// StartButton 会经注入的 on_click 分发（update 返回带实体名）。
    #[test]
    fn ui_test_scene_renders_ui_and_dispatches_clicks() {
        use crate::core::input::MouseButton;
        use crate::core::scene::Scene;

        let root = crate::core::games_dir().join("examples").join("game_api");
        let path = root.join("scenes/ui_test.scene");
        assert!(path.is_file(), "ui_test 示例场景缺失: {}", path.display());
        let scene = Scene::load(&path).unwrap();
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene).unwrap();
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        assert!(!mesh.draws.is_empty(), "ui_test 应渲染出可见 draw");
        let targets = runtime.compute_click_targets();
        assert!(
            targets.len() >= 2,
            "ui_test 应有多个 UiEvents 目标: {targets:#?}"
        );
        let target = targets
            .iter()
            .find(|target| target.name == "StartButton")
            .expect("StartButton UiEvents 目标");
        let script_path =
            std::env::temp_dir().join(format!("p2-ui-test-click-{}.lua", std::process::id()));
        std::fs::write(
            &script_path,
            "function on_click(name) clicked = name end\nfunction update() return tostring(clicked or 'nil') end",
        )
        .unwrap();
        runtime.script.execute_project_script(&script_path).unwrap();
        let cx = target.origin[0] + target.size[0] / 2.0;
        let cy = target.origin[1] + target.size[1] / 2.0;
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
        let _ = std::fs::remove_file(script_path);
    }

    /// 九宫格：带 `nine_slice` 的 Image 应渲染为 3×3 共 9 格（54 顶点），而非
    /// 单四边形（6 顶点）。用真实按钮纹理（192×64）验证 UV 分片路径。
    #[test]
    fn nine_slice_image_renders_nine_quads() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = crate::core::games_dir().join("examples").join("game_api");
        if !root.join("assets/textures/ui/button_green.png").is_file() {
            return;
        }
        let mut scene = Scene::new("nine-slice");
        scene.add_entity(SceneEntityId(1), "Panel", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Image".to_owned(),
                "texture=assets/textures/ui/button_green.png;color=1,1,1;nine_slice=16,16,16,16"
                    .to_owned(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "UiLayout".to_owned(),
                "anchors=0,0,0,0;offsets=0,0,192,64".to_owned(),
            );
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        assert!(
            mesh.draws.iter().any(|d| d.count == 54),
            "切片 Image 应产生 54 顶点（9 格 × 6）：{:#?}",
            mesh.draws.iter().map(|d| d.count).collect::<Vec<_>>()
        );
    }

    /// Scene `Text` layouts already contain absolute screen coordinates. A
    /// regression used to add the `UiLayout` origin a second time, pushing
    /// labels below/off their buttons.
    #[test]
    fn scene_ui_text_keeps_layout_origin_once() {
        use crate::core::scene::{Scene, SceneEntityId};

        let font_source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
        if !font_source.is_file() {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "p2-scene-text-origin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let fonts_dir = root.join("assets").join("fonts");
        fs::create_dir_all(&fonts_dir).unwrap();
        fs::copy(&font_source, fonts_dir.join("NotoSansSC-VF.ttf")).unwrap();

        let mut scene = Scene::new("scene-text-origin");
        scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "UiLayout".to_owned(),
            "anchors=0,0,0,0;offsets=100,600,100,600".to_owned(),
        );
        entity.components.insert(
            "Text".to_owned(),
            "text=Press;font_size=48;color=1,1,1".to_owned(),
        );
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        let _ = fs::remove_dir_all(&root);
        let text_y: Vec<f32> = mesh
            .vector_vertices
            .iter()
            .map(|vertex| vertex.position[1])
            .collect();
        assert!(!text_y.is_empty(), "scene text should emit vector vertices");
        let minimum_y = text_y.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(
            minimum_y > -1.0,
            "layout text must stay on screen, min NDC y={minimum_y}"
        );
    }

    /// Runtime UI layout nodes are keyed by visible-entity order, not by the
    /// sparse authoring ID. Dragging entity 2 must never rewrite entity 1.
    #[test]
    fn ui_layout_preview_targets_authored_entity() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("drag-preview");
        for id in [1, 2] {
            scene
                .add_entity(SceneEntityId(id), format!("Entity{id}"), None)
                .unwrap();
            let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
            entity.components.insert(
                "Text".to_owned(),
                format!("text=Text{id};font_size=24").to_owned(),
            );
            entity.components.insert(
                "UiLayout".to_owned(),
                format!("anchors=0,0,0,0;offsets={id}0,{id}0,{id}0,{id}0"),
            );
        }
        let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
        assert!(
            runtime.set_scene_entity_ui_layout_preview(SceneEntityId(2), [50.0, 60.0, 70.0, 80.0])
        );
        let first = runtime.ui_scene_nodes[&0].layout.as_ref().unwrap().offsets;
        let second = runtime.ui_scene_nodes[&1].layout.as_ref().unwrap().offsets;
        assert_eq!(first, [10.0, 10.0, 10.0, 10.0]);
        assert_eq!(second, [50.0, 60.0, 70.0, 80.0]);
    }

    #[test]
    fn ui_layout_rects_expose_exact_preview_geometry() {
        use crate::core::scene::SceneEntityId;

        let mut scene = Scene::new("drag-preview-geometry");
        scene.add_entity(SceneEntityId(1), "Image", None).unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Image".to_owned(),
            "texture=__white__;color=1,1,1".to_owned(),
        );
        entity.components.insert(
            "UiLayout".to_owned(),
            "anchors=0,0,0,0;offsets=10,20,30,40".to_owned(),
        );
        let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
        assert!(
            runtime.set_scene_entity_ui_layout_preview(SceneEntityId(1), [50.0, 60.0, 90.0, 100.0])
        );
        let rects = runtime.scene_entity_ui_layout_rects([SceneEntityId(1)], [1000.0, 1000.0]);
        assert_eq!(rects.get(&1), Some(&UiRect::new(50.0, 60.0, 40.0, 40.0)));
    }

    /// The editor drag preview must affect the next rendered mesh, not only the
    /// internal layout snapshot.
    #[test]
    fn ui_layout_preview_changes_rendered_mesh() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("drag-preview-render");
        for id in [1, 2] {
            scene
                .add_entity(SceneEntityId(id), format!("Entity{id}"), None)
                .unwrap();
            let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
            entity.components.insert(
                "Image".to_owned(),
                "texture=__white__;color=1,1,1".to_owned(),
            );
            entity.components.insert(
                "UiLayout".to_owned(),
                format!("anchors=0,0,0,0;offsets={id}0,{id}0,{id}0,{id}0"),
            );
        }
        let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
        runtime.set_render_ui_overlay(true);
        let before = runtime.vertices(1000, 1000).unwrap();
        assert!(
            runtime
                .set_scene_entity_ui_layout_preview(SceneEntityId(2), [700.0, 700.0, 740.0, 740.0])
        );
        let after = runtime.vertices(1000, 1000).unwrap();
        let before_x = before
            .vertices
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let after_x = after
            .vertices
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(after_x > before_x);
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

    /// 标题场景（`main.scene`）的手柄导航在**真实 RuntimeScene 路径**上可用。
    ///
    /// 编辑器 Play 预览走的就是这条路径（每帧全新键鼠快照 + `run_script_steps` 合并手柄），
    /// 而 M20-08 的验收示例是直接驱动 `LuaRuntime`，覆盖不到 `input_apply` 这一步。
    #[test]
    fn title_scene_gamepad_navigation_works_through_runtime_scene() {
        use crate::host::gamepad::{GamepadFrame, SlotBackend, button};

        let root = crate::core::games_dir().join("examples").join("game_jhyt");
        let scene_path = root.join("scenes/main.scene");
        if !scene_path.is_file() {
            return;
        }
        let scene = Scene::load(&scene_path).expect("load main.scene");
        let mut runtime =
            RuntimeScene::from_scene(Some(&root), scene).expect("runtime from main.scene");

        let idle = GamepadFrame::idle();
        let mut dpad_down = GamepadFrame::idle();
        dpad_down.buttons[button::DPAD_DOWN] = true;
        // ① 空闲帧（建立设备基线）→ ② 起按住 DPad 下。
        runtime.set_gamepad_backend(Box::new(SlotBackend::new(vec![
            [Some(idle), None, None, None],
            [Some(dpad_down), None, None, None],
        ])));

        let probe = |runtime: &mut RuntimeScene| -> String {
            runtime
                .script
                .execute_console("return tostring(require('ui.title_menu').selected_id())")
                .expect("probe title menu")
                .trim()
                .to_owned()
        };

        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("idle step");
        let before = probe(&mut runtime);
        assert_ne!(before, "nil", "标题菜单应在 main.scene 上初始化");
        for _ in 0..3 {
            runtime
                .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
                .expect("dpad step");
        }
        let after = probe(&mut runtime);
        println!("title selected: before={before} after={after}");
        assert_ne!(after, before, "DPad 下应改变标题菜单选中项");
    }

    /// 标题场景（`main.scene`）的**摇杆**菜单导航在真实 RuntimeScene 路径上可用。
    ///
    /// 项目 `settings.toml` 把左摇杆绑给了 `move_*`（实机测试用），`ui_*` 未列在 `map` 里
    /// → `load_settings` 用默认绑定补齐（含 `padaxis:1±`），因此摇杆仍能选菜单。这条路径
    /// 依赖 **`ui_*` 的轴绑定 + 轴边沿**（M22/W3），与 M20-08 的「Lua 直接读 `pad_axis`」不同。
    #[test]
    fn title_scene_stick_navigation_works_through_runtime_scene() {
        use crate::host::gamepad::{GamepadFrame, SlotBackend};

        let root = crate::core::games_dir().join("examples").join("game_jhyt");
        let scene_path = root.join("scenes/main.scene");
        if !scene_path.is_file() {
            return;
        }
        let scene = Scene::load(&scene_path).expect("load main.scene");
        let mut runtime =
            RuntimeScene::from_scene(Some(&root), scene).expect("runtime from main.scene");

        let mut stick_down = GamepadFrame::idle();
        stick_down.axes[crate::host::gamepad::axis::LEFT_Y] = -0.9;
        runtime.set_gamepad_backend(Box::new(SlotBackend::new(vec![
            [Some(GamepadFrame::idle()), None, None, None],
            [Some(stick_down), None, None, None],
        ])));

        let probe = |runtime: &mut RuntimeScene| -> String {
            runtime
                .script
                .execute_console("return tostring(require('ui.title_menu').selected_id())")
                .expect("probe title menu")
                .trim()
                .to_owned()
        };

        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("idle step");
        let before = probe(&mut runtime);
        for _ in 0..3 {
            runtime
                .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
                .expect("stick step");
        }
        let after = probe(&mut runtime);
        println!("title selected (stick): before={before} after={after}");
        assert_ne!(after, before, "摇杆下推应改变标题菜单选中项");
    }

    /// 编辑器 Play 预览每帧重建输入快照（`bridge_preview_input` 只桥接键鼠），
    /// 而独立 game 宿主跨帧复用同一快照。设备在被按住期间持续上报同一轴值时，
    /// 运行时必须把**当前**手柄状态写进本帧输入，而不是只写「与上一帧不同」的轴——
    /// 否则编辑器预览里摇杆推住不动就等于没有输入（回归：编辑器内摇杆无反应）。
    #[test]
    fn held_gamepad_stick_reaches_script_with_fresh_input_snapshot_per_frame() {
        /// 恒定设备帧：始终「按住」左摇杆上推（值不变 → 无变化边沿）。
        struct HeldStickBackend {
            frame: crate::host::gamepad::GamepadFrame,
        }

        impl crate::host::gamepad::GamepadBackend for HeldStickBackend {
            fn poll(&mut self) -> crate::host::gamepad::GamepadFrame {
                self.frame
            }

            fn name(&self) -> &'static str {
                "held-stick"
            }
        }

        let root = std::env::temp_dir().join(format!(
            "p2-preview-gamepad-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(
            scripts.join("main.lua"),
            "args = {}\n\
             function update()\n\
               local s = input.pad_axis(1)\n\
               if type(s) == 'number' and s == s and s ~= 0 then args[#args + 1] = s end\n\
               return ''\n\
             end\n",
        )
        .unwrap();

        let mut runtime = RuntimeScene::from_scene(Some(&root), Scene::new("preview-gamepad"))
            .expect("runtime scene");
        let mut frame = crate::host::gamepad::GamepadFrame::idle();
        frame.axes[crate::host::gamepad::axis::LEFT_Y] = 0.8;
        runtime.set_gamepad_backend(Box::new(HeldStickBackend { frame }));

        // 与编辑器预览逐帧一致：每帧都是全新快照（不跨帧保留手柄状态）。
        for _ in 0..3 {
            runtime
                .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
                .expect("script step");
        }
        let sampled = runtime
            .script
            .execute_console("return tostring(#args) .. '|' .. tostring(args[1])")
            .expect("probe lua");
        let (count, value) = sampled.trim().split_once('|').expect("count|value");
        assert_eq!(
            count, "3",
            "每帧都应读到被按住摇杆的轴值（编辑器预览逐帧重建快照）"
        );
        let value: f32 = value.parse().expect("axis value");
        assert!(
            (value - 0.8).abs() < 1e-3,
            "轴值应保持设备上报值 0.8：{value}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M22/W4：**0 步帧不得吞掉手柄边沿**。
    ///
    /// 宿主侧键鼠边沿只在 `steps > 0` 时被 `finish_frame` 清掉（`game_app.rs`），所以 0 步帧
    /// （帧率高于逻辑步，关 vsync 时常见）里发生的按键会保留到下一有效帧。手柄此前在 0 步帧
    /// 也会 poll：边沿被写进那个随后丢弃的快照克隆，而 `poller.last` 已推进 → 下一帧不再产生
    /// 同一变化，边沿**永久丢失**。修复后 0 步帧不采样，两条路径语义一致。
    #[test]
    fn zero_step_frame_does_not_drop_gamepad_edges() {
        use crate::host::gamepad::{FakeBackend, GamepadFrame, button};

        let root = std::env::temp_dir().join(format!(
            "p2-zero-step-gamepad-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(
            scripts.join("main.lua"),
            "presses = 0\n\
             function update()\n\
               if input.action_pressed('confirm') then presses = presses + 1 end\n\
               return ''\n\
             end\n",
        )
        .unwrap();

        let mut runtime = RuntimeScene::from_scene(Some(&root), Scene::new("zero-step-gamepad"))
            .expect("runtime scene");
        // 设备从第一帧起就按住 A：唯一一次按下边沿必须被某个**有效**帧看到。
        let mut held = GamepadFrame::idle();
        held.buttons[button::A] = true;
        runtime.set_gamepad_backend(Box::new(FakeBackend::new(vec![held])));

        // 0 步帧（此前会在这里 poll 并丢掉边沿）。
        runtime
            .run_script_steps(0, 1.0 / 60.0, &InputSnapshot::default())
            .expect("zero-step frame");
        // 第一个有效帧：应看到按下边沿。
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("first script step");
        // 第二个有效帧：按住不重复边沿。
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("second script step");

        let count = runtime
            .script
            .execute_console("return tostring(presses)")
            .expect("probe lua");
        assert_eq!(
            count.trim(),
            "1",
            "0 步帧不得吞掉手柄按下边沿；按住也不得重复边沿"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M22/W2：Lua `input.rumble` 的请求在帧末到达设备层（含槽位与强度）。
    #[test]
    fn lua_rumble_reaches_device_layer() {
        use crate::host::gamepad::{GamepadFrame, SlotBackend};

        let root = std::env::temp_dir().join(format!(
            "p2-rumble-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(
            scripts.join("main.lua"),
            "function update()\n\
               input.rumble(0.4, 0.6, 120)\n\
               return ''\n\
             end\n",
        )
        .unwrap();

        let mut runtime =
            RuntimeScene::from_scene(Some(&root), Scene::new("rumble")).expect("runtime scene");
        let backend = SlotBackend::new(vec![[Some(GamepadFrame::idle()), None, None, None]]);
        let log = backend.rumble_log();
        runtime.set_gamepad_backend(Box::new(backend));

        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("script step");
        assert_eq!(
            log.calls().last().copied(),
            Some((0, 0.4, 0.6)),
            "Lua 震动请求应到达设备层"
        );
        // 请求取走后不重复触发（下一帧没有新请求）。
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("script step");
        assert_eq!(log.calls().len(), 2, "每帧一次请求，不做额外重发");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 俄罗斯方块工程冒烟：加载场景 + 脚本，跑几步（输入驱动），验证精灵/UI
    /// 渲染与 Lua 脚本不报错。
    #[test]
    fn tetris_project_runs_and_renders() {
        use crate::core::input::KeyCode;
        use crate::core::scene::Scene;

        let root = crate::core::fixture_games_dir().join("tetris");
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
        input.key_event(KeyCode::KeyD, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step d");
        input.key_event(KeyCode::KeyW, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step w");
        input.key_event(KeyCode::Space, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step space");
        // 多步后仍能渲染。
        let mesh = runtime.vertices(1280, 720).expect("vertices after steps");
        assert!(!mesh.draws.is_empty(), "多步后仍应渲染");
    }

    /// UiEvents 命中矩形：Image 用 UI 矩形、Text 用 SDF 字形 bbox、Sprite 用
    /// 相机投影到屏幕的 AABB。验证三类实体都能得到正确的 `(名字, origin, size)`。
    #[test]
    fn compute_click_targets_resolves_image_text_sprite() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = crate::core::games_dir().join("examples");
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
                ("Image", "texture=assets/textures/coin.png;color=1,1,1"),
                ("UiLayout", "anchors=0,0,0,0;offsets=10,20,74,52"),
                ("UiEvents", "event=click;function=on_click"),
            ],
        );
        add(
            &mut scene,
            2,
            "TxtClick",
            &[
                ("Text", "text=Go;font_size=24;color=1,1,1"),
                ("UiLayout", "anchors=0,0,0,0;offsets=5,5,5,5"),
                ("UiEvents", "event=click;function=on_click"),
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
                ("UiEvents", "event=click;function=on_click"),
            ],
        );
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
        runtime.set_camera_2d(Camera2D::new(Vec2::new(0.0, 0.0), 1.0).expect("default camera"));
        let targets = runtime.compute_click_targets();
        let img = targets
            .iter()
            .find(|target| target.name == "ImgClick")
            .expect("image click target");
        assert_eq!(img.origin, [10.0, 20.0], "Image origin 来自 UiLayout");
        assert_eq!(img.size, [64.0, 32.0], "Image size 来自 UiLayout");
        assert!(
            targets.iter().any(|target| target.name == "TxtClick"),
            "Text click target 存在"
        );
        let spr = targets
            .iter()
            .find(|target| target.name == "SprClick")
            .expect("sprite click target");
        // 相机在原点、viewport 1280×720：sprite(100,50) 32×32 pivot 0.5
        // → 屏幕 AABB 左上 (724, 294)、尺寸 (32, 32)。
        assert_eq!(spr.origin, [724.0, 294.0]);
        assert_eq!(spr.size, [32.0, 32.0]);
    }

    /// E16：命中矩形完全来自四边 Anchor+Offset 布局；Image 只携带视觉属性。
    #[test]
    fn ui_layout_component_defines_click_rect() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = crate::core::games_dir().join("examples");
        let mut scene = Scene::new("layout");
        scene
            .add_entity(SceneEntityId(1), "StretchImg", None)
            .unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Image".to_owned(),
            "texture=assets/textures/coin.png".to_owned(),
        );
        entity.components.insert(
            "UiLayout".to_owned(),
            crate::core::ui_layout::UiRectLayout {
                anchors: [0.0, 0.0, 1.0, 0.0],
                offsets: [10.0, 20.0, -10.0, 80.0],
                ..crate::core::ui_layout::UiRectLayout::default()
            }
            .to_component_str(),
        );
        entity.components.insert(
            "UiEvents".to_owned(),
            crate::core::ui_events::UiEvents::click("on_click").to_component_str(),
        );
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
        let targets = runtime.compute_click_targets();
        let img = targets
            .iter()
            .find(|target| target.name == "StretchImg")
            .expect("layout click target");
        // viewport 1280×720：stretch 水平 → (10, 20, 1260, 60)。
        assert_eq!(img.origin, [10.0, 20.0]);
        assert_eq!(img.size, [1260.0, 60.0]);
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
                world
                    .entities_with::<crate::core::ui_events::UiEvents>()
                    .len(),
                world.entities_with::<UiImage>().len(),
                world.entities_with::<UiText>().len(),
            )
        });
        assert_eq!(counts.0, 2, "menu scene buttons are UiEvents entities");
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
                world
                    .entities_with::<crate::core::ui_events::UiEvents>()
                    .len(),
            )
        });
        assert_eq!(counts.0, 1, "production scene should have one tilemap");
        assert_eq!(
            counts.1, 11,
            "production scene should have player plus ten animated enemies"
        );
        assert_eq!(
            counts.2, 1,
            "production scene exposes one UiEvents (BackToMenuButton)"
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

    /// 场景 Text 指定自定义字体时：该字体 key 出现在 SceneMesh 的自定义图集
    /// 上报与纹理路径里；默认文本在短文本场景改走矢量路径。用临时项目根存放
    /// 两个字体文件（同源，仅验证管线/命名空间，不验证字形差异）。
    #[test]
    fn scene_text_custom_font_surfaces_in_mesh() {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
        if !source.is_file() {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "p2-custom-font-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let fonts_dir = root.join("assets").join("fonts");
        let _ = fs::create_dir_all(&fonts_dir);
        fs::copy(&source, fonts_dir.join("NotoSansSC-VF.ttf")).expect("copy default font");
        fs::copy(&source, fonts_dir.join("Custom.ttf")).expect("copy custom font");

        let mut scene = Scene::new("custom-fonts");
        scene
            .add_entity(SceneEntityId(1), "DefaultText", None)
            .unwrap();
        scene
            .add_entity(SceneEntityId(2), "CustomText", None)
            .unwrap();
        let layout = "anchors=0,0,0,0;offsets=0,0,64,16".to_owned();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("UiLayout".into(), layout.clone());
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Text".into(), "text=Default;font_size=24".into());
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("UiLayout".into(), layout);
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Text".into(),
                "text=Custom;font_size=24;font=assets/fonts/Custom.ttf".into(),
            );

        let result = RuntimeScene::from_scene_edit(Some(&root), scene)
            .and_then(|mut runtime| runtime.vertices(320, 240));
        let _ = fs::remove_dir_all(&root);
        let mesh = result.expect("vertices");
        assert!(
            mesh.custom_font_atlases
                .iter()
                .any(|font| font.key == "font://assets/fonts/Custom.ttf"),
            "custom font key should be reported to the renderer"
        );
        let paths: Vec<&str> = mesh.paths.iter().map(|path| path.as_str()).collect();
        assert!(
            paths.contains(&"font://assets/fonts/Custom.ttf"),
            "custom font path should be in mesh paths: {paths:?}"
        );
        assert!(
            !paths.contains(&FONT_ATLAS_TEXTURE_PATH),
            "default short text should use the vector path: {paths:?}"
        );
        assert!(
            mesh.vector_vertices.len() >= 4,
            "default short text should emit vector vertices"
        );
    }

    #[test]
    fn text_render_mode_forces_vector_or_sdf_for_custom_fonts() {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
        if !source.is_file() {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "p2-text-render-mode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let fonts_dir = root.join("assets").join("fonts");
        fs::create_dir_all(&fonts_dir).unwrap();
        fs::copy(&source, fonts_dir.join("Custom.ttf")).unwrap();

        let mut scene = Scene::new("render-mode");
        scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "UiLayout".into(),
                "anchors=0,0,0,0;offsets=20,20,240,80".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Text".into(),
                "text=Mode;font_size=48;font=assets/fonts/Custom.ttf".into(),
            );

        let project = |mode: &str| {
            fs::write(
                root.join("project.toml"),
                format!(
                    "[project]\nfont = \"assets/fonts/NotoSansSC-VF.ttf\"\ntext_render_mode = \"{mode}\"\n"
                ),
            )
            .unwrap();
        };
        project("vector");
        let vector = RuntimeScene::from_scene_edit(Some(&root), scene.clone())
            .and_then(|mut runtime| runtime.vertices(320, 240))
            .expect("vector vertices");
        assert!(!vector.vector_vertices.is_empty());
        assert!(
            vector.custom_font_atlases.is_empty(),
            "forced vector mode should not build custom SDF atlases"
        );

        project("sdf");
        let sdf = RuntimeScene::from_scene_edit(Some(&root), scene)
            .and_then(|mut runtime| runtime.vertices(320, 240))
            .expect("sdf vertices");
        assert!(sdf.vector_vertices.is_empty());
        assert!(
            sdf.custom_font_atlases
                .iter()
                .any(|font| font.key == "font://assets/fonts/Custom.ttf")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn text_render_mode_parsing_uses_auto_fallback() {
        assert_eq!(TextRenderMode::parse("auto"), TextRenderMode::Auto);
        assert_eq!(TextRenderMode::parse(" Vector "), TextRenderMode::Vector);
        assert_eq!(TextRenderMode::parse("MSDF"), TextRenderMode::Sdf);
        assert_eq!(TextRenderMode::parse("bad"), TextRenderMode::Auto);
    }

    #[test]
    fn sourcehan_sdf_mode_emits_default_font_draws() {
        let root = crate::core::games_dir()
            .join("examples")
            .join("sourcehan_test");
        if !root.join("scenes/font_check.scene").is_file() {
            return;
        }
        let path = root.join("scenes/font_check.scene");
        let scene = Scene::load(&path).unwrap();
        let font_atlas = load_project_font_atlas(Some(&root)).expect("project font atlas");
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
        runtime.set_font_atlas(Some(font_atlas));
        runtime.set_text_render_mode(TextRenderMode::Sdf);
        runtime.set_render_ui_overlay(false);
        let hidden = runtime.vertices(1280, 720).unwrap();
        runtime.set_render_ui_overlay(true);
        let mesh = runtime.vertices(1280, 720).unwrap();
        let sdf_draws = mesh.draws.iter().filter(|draw| draw.sdf).count();
        let font_paths = mesh
            .paths
            .iter()
            .filter(|path| path.as_str() == FONT_ATLAS_TEXTURE_PATH)
            .count();
        assert!(
            sdf_draws > 0 && font_paths > 0,
            "sdf draws={sdf_draws}, font paths={font_paths}, all={:?}",
            mesh.paths
        );
        // 空网格帧（overlay=false）不得消费版本号：首个可绘制帧必须携带全量
        // 像素（否则新渲染器会以空数据 write_texture 整张图集 → wgpu 崩溃）。
        assert!(
            hidden
                .font_atlas
                .as_ref()
                .is_some_and(|font| font.pixels.is_empty()),
            "empty-mesh frame must not consume the atlas version (deferred to drawable frame)"
        );
        assert!(
            mesh.font_atlas
                .as_ref()
                .is_some_and(|font| !font.pixels.is_empty()),
            "first drawable frame must carry full atlas pixels"
        );
    }
}

/// M18-03 共享帧调度管线测试：内置阶段顺序、注册 API、依赖/循环/panic 诊断
/// 与调度性能统计出口。这些测试是 headless 的：直接驱动 RuntimeScene。
#[cfg(test)]
mod frame_schedule_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn sample_root() -> std::path::PathBuf {
        crate::core::games_dir().join("examples").join("game_api")
    }

    fn sample_runtime() -> RuntimeScene {
        let root = sample_root();
        RuntimeScene::new(Some(&root)).expect("runtime for game_api sample project")
    }

    /// 固定步顺序必须与原手工编排一致：
    /// input_apply → lua_update → ui_click_dispatch → physics2d → physics3d →
    /// particles → clock → scene_transition；帧级系统每调用一次：
    /// integrate_velocity → animation_state_advance → animation3d_advance。
    #[test]
    fn builtin_pipeline_preserves_legacy_stage_order() {
        let mut runtime = sample_runtime();
        let order: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let recorder = |name: &'static str, order: Rc<RefCell<Vec<String>>>| {
            move |_scene: &mut RuntimeScene, _dt: f32| {
                order.borrow_mut().push(name.to_owned());
                Ok(())
            }
        };
        runtime
            .add_step_system(System::new(
                "user_script_probe",
                ScheduleStage::Script,
                ["lua_update"],
                recorder("user_script_probe", Rc::clone(&order)),
            ))
            .unwrap();
        runtime
            .add_step_system(System::new(
                "user_physics_probe",
                ScheduleStage::Physics,
                [] as [&str; 0],
                recorder("user_physics_probe", Rc::clone(&order)),
            ))
            .unwrap();
        runtime
            .run_script_steps(2, 1.0 / 60.0, &InputSnapshot::default())
            .expect("scheduled steps");
        // 两个用户探针各自每步执行一次，且 Script 阶段探针先于 Physics 探针。
        assert_eq!(
            order.borrow().as_slice(),
            [
                "user_script_probe",
                "user_physics_probe",
                "user_script_probe",
                "user_physics_probe",
            ]
        );
        // 调度统计的 timings 按「首次执行顺序」记录，完整反映共享管线的
        // 阶段顺序：内置固定步顺序与原手工编排一致，帧级系统在末尾。
        let report = runtime.last_schedule_report().expect("schedule report");
        let names: Vec<&str> = report.timings.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "input_apply",
                "lua_update",
                "ui_click_dispatch",
                "user_script_probe",
                "physics2d_step",
                "physics3d_step",
                "user_physics_probe",
                "particles_step",
                "clock_step",
                "scene_transition",
                "integrate_velocity",
                "animation_state_advance",
                "animation3d_advance",
            ]
        );
    }

    #[test]
    fn schedule_report_collects_steps_and_timings() {
        let mut runtime = sample_runtime();
        runtime
            .run_script_steps(3, 1.0 / 60.0, &InputSnapshot::default())
            .expect("scheduled steps");
        let report = runtime
            .last_schedule_report()
            .expect("schedule report is recorded");
        assert_eq!(report.steps, 3);
        let names: Vec<&str> = report.timings.iter().map(|t| t.name.as_str()).collect();
        for expected in [
            "input_apply",
            "lua_update",
            "ui_click_dispatch",
            "physics2d_step",
            "physics3d_step",
            "particles_step",
            "clock_step",
            "scene_transition",
            "integrate_velocity",
            "animation_state_advance",
            "animation3d_advance",
        ] {
            assert!(names.contains(&expected), "missing timing: {expected}");
        }
        assert!(report.total_elapsed >= report.timings.iter().map(|t| t.elapsed).sum::<Duration>());
        // 帧级系统只执行一次：其累计耗时应远小于每步系统（同量级下不可靠，
        // 仅验证 steps 计数与 timings 覆盖）。
        assert!(report.total_elapsed.as_nanos() > 0);
    }

    #[test]
    fn duplicate_system_names_are_rejected_at_registration() {
        let mut runtime = sample_runtime();
        let error = runtime
            .add_step_system(System::new(
                "lua_update",
                ScheduleStage::Script,
                [] as [&str; 0],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .expect_err("duplicate builtin name must be rejected");
        assert!(matches!(error, SchedulerError::DuplicateSystem { .. }));
        runtime
            .add_frame_system(System::new(
                "frame_probe",
                ScheduleStage::Animation,
                [] as [&str; 0],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .unwrap();
        let error = runtime
            .add_step_system(System::new(
                "frame_probe",
                ScheduleStage::Transform,
                [] as [&str; 0],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .expect_err("cross-pipeline duplicate name must be rejected");
        assert!(matches!(error, SchedulerError::DuplicateSystem { .. }));
    }

    /// 回归：注册失败绝不能"吃掉"已经注册的系统。
    ///
    /// 旧实现每帧把系统 `drain` 进新调度器、遇到 `add_system` 失败就 `break`：
    /// `Drain` 在提前退出时会连同**尚未搬迁**的系统一起丢弃，一次失败的注册就能
    /// 让后续的内建系统（物理、时钟、动画）静默消失。现在系统所有权始终在调度器
    /// 里，被拒绝的注册只影响它自己。
    #[test]
    fn rejected_registration_never_drops_registered_systems() {
        let mut runtime = sample_runtime();
        let (step_names, frame_names) = runtime.registered_system_names();
        let builtin_steps = step_names.len();
        assert!(builtin_steps >= 8, "builtin step systems: {step_names:?}");
        assert!(!frame_names.is_empty());

        // 重名的注册必须被拒绝。
        let error = runtime
            .add_step_system(System::new(
                "lua_update",
                ScheduleStage::Script,
                [] as [&str; 0],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .expect_err("duplicate name must be rejected");
        assert!(matches!(error, SchedulerError::DuplicateSystem { .. }));

        // 被拒绝之后：注册表一个不少，调度仍然完整跑通。
        let (step_names_after, frame_names_after) = runtime.registered_system_names();
        assert_eq!(step_names_after, step_names);
        assert_eq!(frame_names_after, frame_names);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("schedule must still run after a rejected registration");
        let names: Vec<&str> = runtime
            .last_schedule_report()
            .expect("schedule report is recorded")
            .timings
            .iter()
            .map(|timing| timing.name.as_str())
            .collect();
        for expected in [
            "input_apply",
            "lua_update",
            "physics2d_step",
            "clock_step",
            "integrate_velocity",
        ] {
            assert!(names.contains(&expected), "dropped system: {expected}");
        }
    }

    #[test]
    fn missing_dependency_is_diagnosed_with_system_names() {
        let mut runtime = sample_runtime();
        runtime
            .add_step_system(System::new(
                "orphan_probe",
                ScheduleStage::Script,
                ["no_such_system"],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .unwrap();
        let error = runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect_err("missing dependency must fail the run");
        assert!(
            error.contains("orphan_probe") && error.contains("no_such_system"),
            "diagnostic must name system and dependency: {error}"
        );
        // 失败后注册仍在（系统已放回），修复依赖后可重新执行。
        let (step_names, _) = runtime.registered_system_names();
        assert!(step_names.contains(&"orphan_probe".to_owned()));
    }

    #[test]
    fn dependency_cycle_is_diagnosed_with_the_cycle_members() {
        let mut runtime = sample_runtime();
        runtime
            .add_step_system(System::new(
                "cycle_a",
                ScheduleStage::Physics,
                ["cycle_b"],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .unwrap();
        runtime
            .add_step_system(System::new(
                "cycle_b",
                ScheduleStage::Physics,
                ["cycle_a"],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .unwrap();
        let error = runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect_err("dependency cycle must fail the run");
        assert!(
            error.contains("cycle_a") && error.contains("cycle_b"),
            "cycle diagnostic must name the members: {error}"
        );
    }

    #[test]
    fn system_panic_is_diagnosed_and_registration_survives() {
        let mut runtime = sample_runtime();
        runtime
            .add_step_system(System::new(
                "panic_probe",
                ScheduleStage::Transform,
                [] as [&str; 0],
                |_: &mut RuntimeScene, _| panic!("probe-boom"),
            ))
            .unwrap();
        let error = runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect_err("panicking system must fail the run");
        assert!(
            error.contains("panic_probe") && error.contains("probe-boom"),
            "panic diagnostic must name system and payload: {error}"
        );
        let (step_names, _) = runtime.registered_system_names();
        assert!(step_names.contains(&"panic_probe".to_owned()));
    }

    #[test]
    fn systems_owning_script_state_run_inside_the_shared_schedule() {
        // 通过 with_script 在注册系统内驱动引擎脚本 API（Rust 侧玩法系统
        // 的官方接入路径），并验证 frame 系统每次调用只执行一次。
        let mut runtime = sample_runtime();
        let runs = Rc::new(RefCell::new(0u32));
        let counter = Rc::clone(&runs);
        runtime
            .add_frame_system(System::new(
                "frame_counter_probe",
                ScheduleStage::Transform,
                [] as [&str; 0],
                move |scene: &mut RuntimeScene, _dt: f32| {
                    *counter.borrow_mut() += 1;
                    scene.with_script(|script| {
                        script.set_delta(0.0);
                    });
                    Ok(())
                },
            ))
            .unwrap();
        runtime
            .run_script_steps(4, 1.0 / 60.0, &InputSnapshot::default())
            .expect("scheduled steps");
        assert_eq!(*runs.borrow(), 1, "frame systems run once per call");
        runtime
            .run_script_steps(0, 1.0 / 60.0, &InputSnapshot::default())
            .expect("zero-step call still runs the frame pipeline");
        assert_eq!(*runs.borrow(), 2);
    }
}
