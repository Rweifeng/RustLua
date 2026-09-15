use std::{
    cell::RefCell,
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::SystemTime,
};

use mlua::{Function, Lua, Result as LuaResult, Table, Value};
use tracing::{error, info, warn};

use crate::core::{
    animation::AnimationStateMachine,
    animation3d::{Animation3D, Animation3dDescriptor},
    audio::{AudioBus, AudioSystem},
    camera3d::Camera3D,
    entity::EntityId,
    gameplay::{GameClock, Inventory, ItemDefinition, ItemStack},
    grid::{self, Facing},
    input::{ActionMap, Binding, InputSnapshot, KeyCode, MouseButton},
    light::Light,
    math3d::Vec3,
    mesh3d::MeshRenderer,
    particle::{Particle, ParticleSystem},
    physics::{ColliderDesc, ColliderShape, PhysicsWorld},
    physics3d::{ColliderDesc3d, PhysicsWorld3d},
    resource_db::AssetId,
    save::encode as encode_save,
    scene::{Scene, SceneEntityId},
    settings::NavSettings,
    settings::ProjectSettings,
    sprite::{SpriteAsset, SpriteComponent},
    tilemap::DEFAULT_TILESET as DEFAULT_TILESET_PATH,
    tilemap::Tilemap,
    transform::{Transform, Vec2},
    transform3d::Transform3D,
    ui::{DynamicUiImage, UiGrid, UiGridCell, UiPanel},
    ui_events::{UiClickTarget, UiEvents},
    ui_layout::UiRectLayout,
    ui_navigation::{FocusItem, FocusRing, NavDirection, ScrollState},
    world::{World, WorldError},
};

/// `transform3d.set(id, x, y, z, [rx, ry, rz, rw, [sx, sy, sz]])` argument tuple.
type Transform3dSetArgs = (
    i64,
    f32,
    f32,
    f32,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
);

#[derive(Debug, Default)]
struct ScriptState {
    delta_seconds: f32,
    quit_requested: bool,
    input: InputSnapshot,
    world: World,
    names: BTreeMap<String, EntityId>,
    health: BTreeMap<EntityId, f32>,
    /// Stable project-level entity IDs (`persist_id` → live entity). Distinct
    /// from `names` (display/authoring name, collision-prone): a persist ID is
    /// unique across the runtime and is what save/snapshot uses to re-bind
    /// runtime entities (crops, buildings, containers, NPCs) without
    /// duplicating or misbinding them.
    persist: BTreeMap<String, EntityId>,
    scene: Option<String>,
    scene_pending_load: Option<String>,
    project_root: Option<PathBuf>,
    /// 懒加载的字体度量探针（`ui.measure_text` 用）：只读覆盖与 advance、
    /// 不光栅化字形、不占渲染图集内存。`None` = 从未尝试加载。
    font_probe: Option<Option<crate::core::font::FontProbe>>,
    save_path: Option<PathBuf>,
    /// 玩家设置根目录（P3）：发布包 → 用户数据目录（`%APPDATA%\<app>`），
    /// 开发工程 → 项目根（编辑器与验收样例直接检查工程内的 `settings.toml`）。
    /// 未设置时退回 `project_root`。
    settings_root: Option<PathBuf>,
    /// 上一次 Lua `update()` 的耗时（微秒）。宿主把它累加进 `PerformanceStats::lua_ms`
    /// ——这个指标此前只有单测在写，真实运行恒为 `0.000`（P4 接线，见 `docs/37` 5-6）。
    last_update_micros: u64,
    assets: BTreeMap<String, String>,
    ui: BTreeMap<String, UiText>,
    ui_bars: BTreeMap<String, UiBar>,
    save_data: BTreeMap<String, String>,
    physics: PhysicsWorld,
    physics3d: PhysicsWorld3d,
    particles: ParticleSystem,
    audio: AudioSystem,
    /// Entity whose `Camera3D` component the renderer uses this frame. Lua
    /// selects it with `camera.set_active`; the renderer falls back to the
    /// first camera entity when unset.
    active_camera: Option<EntityId>,
    /// Entity whose `Light` component the renderer uses this frame.
    active_light: Option<EntityId>,
    /// Persistent entities (name → authored snapshot) that survive scene loads.
    persistent: BTreeMap<String, PersistedEntity>,
    /// Per-entity string tags used by interact queries and gameplay scripts.
    tags: BTreeMap<EntityId, BTreeMap<String, String>>,
    /// Optional on-screen interaction prompt text (`interact.prompt`).
    interact_prompt: Option<String>,
    game_clock: GameClock,
    item_defs: BTreeMap<String, ItemDefinition>,
    inventories: BTreeMap<String, Inventory>,
    ui_panels: BTreeMap<String, UiPanel>,
    ui_images: BTreeMap<String, DynamicUiImage>,
    ui_grids: BTreeMap<String, UiGrid>,
    data_cache: BTreeMap<String, serde_json::Value>,
    /// Registered `data.schema(path, spec)` validators for data tables.
    schema_defs: BTreeMap<String, serde_json::Value>,
    /// 注册 `map.schema(name, spec)` 的分地图持久状态校验器（B-3）。
    /// 按地图名索引（而不是按场景索引），并刻意跨过
    /// `process_scene_transition` 保留；`schema_defs` 则只跟踪数据文件校验器。
    map_state_schemas: BTreeMap<String, serde_json::Value>,
    /// Shop catalogs: name -> `{ item_id: { buy, sell } }` (M16-10).
    shops: BTreeMap<String, serde_json::Value>,
    save_slot: Option<String>,
    ui_viewport: [f32; 2],
    /// Canvas 空间 UI 参考分辨率（设计分辨率），与运行时渲染保持一致，
    /// 供 Lua 侧 `process_ui_clicks` / `grid_hit` 命中测试使用。
    ui_reference: [f32; 2],
    ui_drag: Option<UiDrag>,
    /// 运行时 UI 焦点环（M20-02）：`ui.focus_open/move/...` 按稳定 id 持有，
    /// 面板用方向键/摇杆方向驱动选择（`core/ui_navigation.rs::FocusRing`）。
    focus_rings: BTreeMap<String, FocusRing>,
    /// 运行时列表滚动状态（M20-02）：面板把选中行滚入视口
    /// （`core/ui_navigation.rs::ScrollState`）。
    scroll_states: BTreeMap<String, ScrollState>,
    /// Authoring ID → live ECS handle. The editor uses this to preview a drag
    /// by mutating one component without rebuilding the whole runtime.
    scene_entities: BTreeMap<SceneEntityId, EntityId>,
    /// 命名动作映射（M20-07）：**唯一实例**随 `ScriptState` 走，脚本重绑定
    /// （`input.rebind`/`set_action_map`/设置文件）与宿主查询同源；默认绑定见
    /// [`ActionMap::with_defaults`]。刻意**不**随场景切换或读档重置——绑定是
    /// 玩家偏好，唯一重置入口是 `input.reset_bindings()`。
    ///
    /// 注意：本结构体的 `Default` 由 derive 生成（约 40 个字段逐一手写易漏），
    /// 而 [`ActionMap::default`] 是**空**映射；因此 [`LuaRuntime::new`] 建好状态
    /// 后立刻 `merge_missing(with_defaults())` 补齐默认绑定（`merge_missing`
    /// 语义为空表 → 完整默认表），脚本与测试必须经 `LuaRuntime` 构造才拿到状态。
    action_map: ActionMap,
    /// 输入层诊断（M20-07，最近 16 条，最新在后）：设置文件回退默认、版本头
    /// 不符、非法绑定、写盘失败等都在这里留下可定位的一行说明，脚本可用
    /// `input.diagnostics()` 读出。不参与存档。
    input_diagnostics: Vec<String>,
    /// **v1 → v2 改键迁移诊断**（M22/W1）：`input.load_settings()` / `set_action_map`
    /// 读入 v1（或 v1 写法）的绑定时，每条**有损换算**各一行中文说明
    /// （`key:+` 只能落到 `key:equal` 这类），脚本用 `input.migration_notes()` 读出。
    /// 与 `input_diagnostics` 分开：诊断是「出错了」，迁移说明是「变了但没丢」。
    input_migration_notes: Vec<String>,
    /// **逻辑时间**（毫秒，M22/W3）：按固定步在 [`ScriptState::set_input`] 里累加
    /// （每步 + `delta_seconds`），供 `input.action_repeat` 做时间制连发。刻意不用
    /// `game_clock`（它受游戏内时间倍率/暂停影响，菜单导航必须始终按真实逻辑时间走）。
    logical_ms: f64,
    /// 菜单导航手感（阈值 + 重复节奏，M22/W3）：默认 [`NavSettings::DEFAULT`]，
    /// 由 `input.load_settings()`（读 `[input]` 段）或 Lua `input.set_nav_settings`
    /// 覆盖；`RuntimeScene` 每帧把 `threshold` 同步给设备轮询器（轴判定阈值同源）。
    nav: NavSettings,
    /// 每个动作的按住/连发状态（M22/W3，时间制）：动作名 → 状态。
    action_repeat: BTreeMap<String, ActionRepeatState>,
    /// 待处理的震动请求（M22/W2）：Lua `input.rumble(low, high, ms)` 写入，宿主在帧末取走
    /// 并交给设备层（`GamepadPoller::rumble`）。同一逻辑帧内多次调用以最后一次为准。
    rumble_request: Option<(f32, f32, u32)>,
}

/// `input.action_repeat` 的每动作状态（M22/W3）。
///
/// * `holding`：上一帧该动作是否处于按下状态（用于识别「首次按下 = 立即一步」）；
/// * `next_repeat_ms`：下一次允许连发的逻辑时间。
#[derive(Debug, Clone, Copy, Default)]
struct ActionRepeatState {
    holding: bool,
    next_repeat_ms: f64,
}

impl ScriptState {
    /// 懒加载字体度量探针（`ui.measure_text`）：要工程根才能找到 `project.toml`
    /// 里的字体配置，故第一次调用才读盘；失败结果也会缓存（`Some(None)`），
    /// 避免每帧重复尝试读一个不存在的字体。切工程（`set_project_root`）会清缓存。
    fn font_probe(&mut self) -> Result<Option<crate::core::font::FontProbe>, LuaBridgeError> {
        if let Some(cached) = &self.font_probe {
            return Ok(cached.clone());
        }
        let root = self
            .project_root
            .clone()
            .ok_or_else(|| LuaBridgeError::Api("no project root configured".to_owned()))?;
        let probe = crate::core::font::project_font::load_probe(&root).ok();
        self.font_probe = Some(probe.clone());
        Ok(probe)
    }
    /// 时间制连发（M22/W3）——[`LuaRuntime::action_repeat`] 与 Lua `input.action_repeat`
    /// 同源实现。详见方法内注释与 `docs/35`。
    fn action_repeat(&mut self, name: &str) -> bool {
        let now = self.logical_ms;
        let down = self.input.action_is_down(&self.action_map, name);
        // 立即走一步只认**真实按下边沿**：轴与键都有边沿（M22），而「已经按住、但本函数
        // 第一次观察到」不能补一步——否则同一根摇杆同时驱动 `move_*` 与 `ui_*` 时，
        // 消费了边沿的那一帧走一步、下一帧 ui 路径又补一步（双步）。
        let edge = self.input.action_just_pressed(&self.action_map, name);
        // 连发只对「持续保持的模拟量绑定」生效（数字键/DPad 保持一次按下 = 一步）。
        let analog = down && self.input.action_held_by_analog(&self.action_map, name);
        let delay_ms = f64::from(self.nav.delay_ms);
        let repeat_ms = f64::from(self.nav.repeat_ms);
        let entry = self.action_repeat.entry(name.to_owned()).or_default();
        if !down {
            entry.holding = false;
            // 边沿 = 「刚按下」，与「此刻是否仍按住」无关：**一步之内完成的「按下 + 松开」**
            // 同样必须走一步——极短点按、编辑器 Play 预览把同帧内的按下/松开发成 down+up
            // 边沿（`game_view::bridge_preview_input`）、摇杆越过阈值后立刻回中都是这种形态。
            // 此前这里无条件 `return false`，于是一步内完成的点按被整帧丢掉，表现为
            // 「按方向键偶发没有反应」。边沿在逻辑步内保留（与 `action_pressed` 同语义），
            // 宿主在每帧首个固定步之后 `finish_frame` 清边沿，因此不会跨步重复。
            return edge;
        }
        if !entry.holding {
            // 开始计时；边沿帧立即走一步，非边沿帧只武装计时器。
            entry.holding = true;
            entry.next_repeat_ms = now + delay_ms;
            return edge;
        }
        if !analog {
            return false;
        }
        if now >= entry.next_repeat_ms {
            // 以「当前时间 + 间隔」推进，而不是累加：长卡顿后不爆发式连发。
            entry.next_repeat_ms = now + repeat_ms;
            return true;
        }
        false
    }
}

/// State for a Lua-driven inventory drag (`ui.drag_begin` / `ui.drag_end`).
#[derive(Clone, Debug)]
struct UiDrag {
    from_id: String,
    from_index: usize,
}

#[derive(Debug, Clone)]
pub struct UiText {
    pub text: String,
    pub visible: bool,
    pub position: [f32; 2],
    /// 像素字号（SDF em 高度，px）。提供时优先于 `scale`；
    /// 换算见 [`crate::core::ui::ui_text_effective_scale`]。
    pub font_size: Option<f32>,
    /// 字形缩放系数（`1.0` = SDF 字体 em 12px / 位图 5x7 字形 7px）。
    pub scale: f32,
}

/// A screen-space filled bar (e.g. a health bar) rendered by the host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiBar {
    pub position: [f32; 2],
    pub size: [f32; 2],
    /// Fill ratio in `0..=1`.
    pub ratio: f32,
    pub color: [f32; 4],
}

/// Authored snapshot of an entity marked `metadata["persistent"]="true"`.
/// Keyed by entity name (the only handle stable across scene switches); kept in
/// `ScriptState::persistent` so `process_scene_transition` can re-spawn it.
#[derive(Clone, Debug, Default)]
struct PersistedEntity {
    name: String,
    components: BTreeMap<String, String>,
    resources: BTreeMap<String, AssetId>,
    /// 保留元数据（如 persistent 标志自身）；当前恢复只用 components。
    #[allow(dead_code)]
    metadata: BTreeMap<String, String>,
}

/// Errors produced while loading or running a project Lua script.
#[derive(Debug)]
pub enum LuaBridgeError {
    Init(String),
    Load { path: PathBuf, source: String },
    Runtime { path: PathBuf, source: String },
    Api(String),
}

impl fmt::Display for LuaBridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Init(source) => write!(f, "Lua VM initialization failed: {source}"),
            Self::Load { path, source } => write!(f, "failed to load {}: {source}", path.display()),
            Self::Runtime { path, source } => {
                write!(f, "script {} failed: {source}", path.display())
            }
            Self::Api(source) => write!(f, "Lua API error: {source}"),
        }
    }
}

impl std::error::Error for LuaBridgeError {}

/// Result of checking a loaded script for source changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReloadStatus {
    Unchanged,
    Reloaded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScriptFingerprint {
    modified: Option<SystemTime>,
    length: u64,
    hash: u64,
}

/// A structured diagnostic emitted by a Lua script load or execution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptDiagnostic {
    pub path: PathBuf,
    pub phase: String,
    pub message: String,
    pub line: Option<u32>,
    pub stack_trace: String,
    pub occurrences: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ScriptDebugger {
    records: Vec<ScriptDiagnostic>,
    filter: Option<String>,
}

impl ScriptDebugger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, path: PathBuf, phase: impl Into<String>, source: impl Into<String>) {
        let phase = phase.into();
        let source = source.into();
        let line = find_line_number(&source);
        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|item| item.path == path && item.phase == phase && item.message == source)
        {
            existing.occurrences = existing.occurrences.saturating_add(1);
            return;
        }
        self.records.push(ScriptDiagnostic {
            path,
            phase,
            message: source.clone(),
            line,
            stack_trace: source,
            occurrences: 1,
        });
    }

    pub fn clear(&mut self) {
        self.records.clear();
    }

    pub fn set_filter(&mut self, filter: Option<String>) {
        self.filter = filter;
    }

    #[must_use]
    pub fn all(&self) -> &[ScriptDiagnostic] {
        &self.records
    }

    #[must_use]
    pub fn visible(&self) -> Vec<ScriptDiagnostic> {
        self.records
            .iter()
            .filter(|item| {
                self.filter.as_ref().is_none_or(|filter| {
                    item.message.contains(filter) || item.path.to_string_lossy().contains(filter)
                })
            })
            .cloned()
            .collect()
    }
}

/// Main-thread Lua runtime for project scripts.
pub struct LuaRuntime {
    lua: Lua,
    state: Rc<RefCell<ScriptState>>,
    current_path: Option<PathBuf>,
    fingerprint: Option<ScriptFingerprint>,
    debugger: ScriptDebugger,
}
impl LuaRuntime {
    pub fn new() -> Result<Self, LuaBridgeError> {
        let lua = Lua::new();
        let state = Rc::new(RefCell::new(ScriptState::default()));
        state.borrow_mut().ui_reference = crate::core::ui::UI_REFERENCE_SIZE;
        // M20-07：空映射 + 默认表 = 完整默认绑定（见 `ScriptState::action_map`）。
        state
            .borrow_mut()
            .action_map
            .merge_missing(&ActionMap::with_defaults());
        register_api(&lua, Rc::clone(&state))
            .map_err(|error| LuaBridgeError::Init(error.to_string()))?;
        Ok(Self {
            lua,
            state,
            current_path: None,
            fingerprint: None,
            debugger: ScriptDebugger::new(),
        })
    }

    /// 当前命名动作映射（M20-07，只读快照）。
    ///
    /// 宿主需要「按动作查询输入」时用它与同一帧的 [`InputSnapshot`] 组合
    /// （`InputSnapshot::action_is_down(&map, "interact")`），保证与脚本侧
    /// `input.action("interact")` 结论一致。
    #[must_use]
    pub fn action_map(&self) -> ActionMap {
        self.state.borrow().action_map.clone()
    }

    /// 注入命名动作映射（M20-07）：宿主设置界面改键、按工程设置文件启动时用，
    /// 之后脚本查询/重绑定都作用在这一份上。
    pub fn set_action_map(&mut self, map: ActionMap) {
        self.state.borrow_mut().action_map = map;
    }

    /// 输入层诊断（M20-07，最新在后）：设置文件回退默认、版本头不符、非法绑定、
    /// 写盘失败等。与 `ScriptDiagnostic`（脚本错误）分开，避免污染调试器记录。
    #[must_use]
    pub fn input_diagnostics(&self) -> Vec<String> {
        self.state.borrow().input_diagnostics.clone()
    }

    pub fn set_delta(&mut self, delta_seconds: f32) {
        self.state.borrow_mut().delta_seconds = delta_seconds.max(0.0);
    }

    pub fn set_input(&mut self, input: InputSnapshot) {
        let mut state = self.state.borrow_mut();
        // M22/W3：逻辑时间按**每个固定步**累加（`set_input` 每步一次），
        // 因此 catch-up 帧（一帧多步）也走同样的时间线，连发节奏与帧率无关。
        state.logical_ms += f64::from(state.delta_seconds) * 1000.0;
        state.input = input;
    }

    /// 当前菜单导航手感（M22/W3；`RuntimeScene` 每帧读它同步设备层阈值）。
    #[must_use]
    pub fn nav_settings(&self) -> NavSettings {
        self.state.borrow().nav
    }

    /// 覆盖菜单导航手感（Lua `input.set_nav_settings` / `load_settings` 用）。
    pub fn set_nav_settings(&mut self, nav: NavSettings) {
        self.state.borrow_mut().nav = nav;
    }

    /// 取走待处理的震动请求（M22/W2；宿主帧末调用一次）。
    ///
    /// Lua `input.rumble(low, high, ms)` 只记录请求，真正的设备调用（含到期归零的状态机）
    /// 在 `host::gamepad::GamepadPoller`，因此需要宿主这一跳。
    pub fn take_rumble_request(&mut self) -> Option<(f32, f32, u32)> {
        self.state.borrow_mut().rumble_request.take()
    }

    /// `input.action_repeat` 的引擎实现（M22/W3，时间制连发）。
    ///
    /// 语义（与 M20-08 的 Lua 帧计数等价，但按毫秒）：
    /// * 动作**本帧刚按下**（键或摇杆轴越过阈值的边沿）→ 立即返回 `true` 一步；
    /// * 之后只有**模拟量绑定**（摇杆轴）持续按住时，才在超过 `nav.delay_ms` 后每
    ///   `nav.repeat_ms` 返回一次 `true`；数字量绑定（键盘/DPad）按住**不连发**——
    ///   保持 M20-08 既有的「一次按下 = 一步」语义（见 `docs/35` 与
    ///   `jhyt_m20_gamepad` 的验收）；
    /// * 「已按住但没有边沿」不补步，只开始计时（避免同一根摇杆同时驱动 `move_*`
    ///   与 `ui_*` 时双步）。
    pub fn action_repeat(&mut self, name: &str) -> bool {
        self.state.borrow_mut().action_repeat(name)
    }

    /// Spawns every scene entity into the runtime world, parsing the editor's
    /// `Transform` and `Sprite` component strings and registering names so Lua
    /// can look entities up with `entity.find`. The host renderer reads the very
    /// same world through [`LuaRuntime::with_world`], so script-driven movement
    /// is immediately visible.
    pub fn instantiate_scene(&mut self, scene: &Scene) -> Result<usize, LuaBridgeError> {
        self.instantiate_scene_with_editor_visibility(scene, false)
    }

    pub fn instantiate_scene_with_editor_visibility(
        &mut self,
        scene: &Scene,
        honor_editor_hidden: bool,
    ) -> Result<usize, LuaBridgeError> {
        let mut state = self.state.borrow_mut();
        state.scene = Some(scene.name.clone());
        state.scene_entities.clear();
        let mut count = 0;
        for entity in scene.entities.values() {
            if honor_editor_hidden && scene.is_entity_hidden_with_ancestors(entity.id) {
                continue;
            }
            let handle = Self::spawn_entity_components(
                &mut state,
                &entity.name,
                &entity.components,
                &entity.resources,
            )?;
            state.scene_entities.insert(entity.id, handle);
            // 持久实体快照（权威、静态组件），供切场景后重生。
            if entity
                .metadata
                .get("persistent")
                .is_some_and(|value| value == "true")
                && !entity.name.is_empty()
            {
                state.persistent.insert(
                    entity.name.clone(),
                    PersistedEntity {
                        name: entity.name.clone(),
                        components: entity.components.clone(),
                        resources: entity.resources.clone(),
                        metadata: entity.metadata.clone(),
                    },
                );
            }
            count += 1;
        }
        Ok(count)
    }

    /// Dispatches click bindings from structured `UiEvents`. Hit targets are
    /// precomputed by `RuntimeScene::compute_click_targets`; each target may
    /// declare one or more Lua callbacks.
    pub fn process_ui_clicks(
        &self,
        input: &InputSnapshot,
        targets: &[UiClickTarget],
    ) -> Result<(), LuaBridgeError> {
        if !input.mouse_just_pressed(MouseButton::Left) {
            return Ok(());
        }
        let (mx, my) = input.cursor_position();
        let clicked: Vec<&UiClickTarget> = targets
            .iter()
            .filter(|target| {
                target.origin[0] <= mx as f32
                    && (mx as f32) < target.origin[0] + target.size[0]
                    && target.origin[1] <= my as f32
                    && (my as f32) < target.origin[1] + target.size[1]
            })
            .collect();
        for target in clicked {
            for function_name in &target.functions {
                let callback: Value =
                    self.lua
                        .globals()
                        .get(function_name.as_str())
                        .map_err(|error| LuaBridgeError::Runtime {
                            path: self
                                .current_path
                                .clone()
                                .unwrap_or_else(|| PathBuf::from("<project script>")),
                            source: error.to_string(),
                        })?;
                if let Value::Function(function) = callback {
                    function
                        .call::<()>((target.name.as_str(),))
                        .map_err(|error| LuaBridgeError::Runtime {
                            path: self
                                .current_path
                                .clone()
                                .unwrap_or_else(|| PathBuf::from("<project script>")),
                            source: error.to_string(),
                        })?;
                }
            }
        }
        Ok(())
    }

    /// Spawns a single entity from its component map into the runtime world
    /// (components, physics bodies, name registration). Single source of truth
    /// used by `instantiate_scene` and by the persistent-entity restore.
    fn spawn_entity_components(
        state: &mut ScriptState,
        name: &str,
        components: &BTreeMap<String, String>,
        _resources: &BTreeMap<String, AssetId>,
    ) -> Result<EntityId, LuaBridgeError> {
        let id = state.world.spawn();
        if let Some(value) = components.get("Transform") {
            let transform = Transform::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Transform: {error}"))
            })?;
            state
                .world
                .insert(id, transform)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(raw) = components.get("Sprite") {
            let component = SpriteComponent::from_component_str(raw).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Sprite: {error}"))
            })?;
            let sprite = component.sprite().map_err(|error| {
                LuaBridgeError::Api(format!(
                    "failed to build sprite for entity {name:?}: {error}"
                ))
            })?;
            state
                .world
                .insert(id, sprite)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            state
                .world
                .insert(id, SpriteAsset(component.texture))
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            state
                .world
                .insert(id, component.sorting)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Tilemap") {
            let tilemap = Tilemap::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Tilemap: {error}"))
            })?;
            state
                .world
                .insert(id, tilemap)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Animation") {
            let animation = AnimationStateMachine::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Animation: {error}"))
            })?;
            state
                .world
                .insert(id, animation)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Transform3D") {
            let transform = Transform3D::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} has an invalid Transform3D: {error}"
                ))
            })?;
            state
                .world
                .insert(id, transform)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Animation3D") {
            let descriptor = Animation3dDescriptor::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} has an invalid Animation3D: {error}"
                ))
            })?;
            let root = state.project_root.clone().ok_or_else(|| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} needs a project root to load its Animation3D mesh"
                ))
            })?;
            let animation = Animation3D::from_gltf_path(&root, &descriptor).map_err(|error| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} Animation3D failed to load: {error}"
                ))
            })?;
            state
                .world
                .insert(id, animation)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("MeshRenderer") {
            let renderer = MeshRenderer::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} has an invalid MeshRenderer: {error}"
                ))
            })?;
            state
                .world
                .insert(id, renderer)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Camera3D") {
            let camera = Camera3D::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Camera3D: {error}"))
            })?;
            state
                .world
                .insert(id, camera)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Light") {
            let light = Light::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Light: {error}"))
            })?;
            state
                .world
                .insert(id, light)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        // UI scene components (screen-space overlays).
        if let Some(value) = components.get("Text") {
            let text = crate::core::ui::UiText::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Text: {error}"))
            })?;
            state
                .world
                .insert(id, text)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Image") {
            let image = crate::core::ui::UiImage::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Image: {error}"))
            })?;
            state
                .world
                .insert(id, image)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(raw) = components.get("UiEvents") {
            let events = UiEvents::parse(raw).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has invalid UiEvents: {error}"))
            })?;
            state
                .world
                .insert(id, events)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        // E16：可选 `UiLayout` 组件——四边 Anchor+Offset 布局，存在时优先于
        // Text/Image 的内联 x/y/width/height/anchor（编辑器与运行时同一 resolve）。
        if let Some(value) = components.get("UiLayout") {
            let layout = UiRectLayout::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid UiLayout: {error}"))
            })?;
            state
                .world
                .insert(id, layout)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if !name.is_empty() {
            state.names.insert(name.to_owned(), id);
        }
        let position = state
            .world
            .get::<Transform>(id)
            .ok()
            .flatten()
            .map(|transform| transform.position)
            .unwrap_or_default();
        if let Some(value) = components.get("Collider") {
            let collider = ColliderDesc::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Collider: {error}"))
            })?;
            state
                .physics
                .add_body(id, position, true, vec![collider])
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if components.contains_key("Tilemap") {
            let tilemap = state.world.get::<Tilemap>(id).ok().flatten();
            if let Some(tilemap) = tilemap
                && tilemap.solid.iter().any(|solid| *solid)
            {
                let colliders = solid_cell_colliders(tilemap);
                state
                    .physics
                    .add_body(id, position, false, colliders)
                    .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            }
        }
        if let Some(value) = components.get("Collider3d") {
            let collider = ColliderDesc3d::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} has an invalid Collider3d: {error}"
                ))
            })?;
            let position = state
                .world
                .get::<Transform3D>(id)
                .ok()
                .flatten()
                .map(|transform| transform.position)
                .unwrap_or(Vec3::ZERO);
            state
                .physics3d
                .add_body(id, position, collider.dynamic, vec![collider])
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        Ok(id)
    }

    /// Steps the physics world and writes moved dynamic body positions back to
    /// their ECS Transforms so rendering and Lua see the simulated result.
    pub fn step_physics(&mut self, delta_seconds: f32) {
        let mut state = self.state.borrow_mut();
        let moved = state.physics.step(delta_seconds);
        for (entity, position) in moved {
            if let Ok(Some(transform)) = state.world.get_mut::<Transform>(entity) {
                transform.position = position;
            }
        }
    }

    /// Steps the 3D physics world and writes moved dynamic body positions back
    /// to their ECS `Transform3D` so rendering and Lua see the simulation.
    pub fn step_physics3d(&mut self, delta_seconds: f32) {
        let mut state = self.state.borrow_mut();
        let moved = state.physics3d.step(delta_seconds);
        for (entity, position) in moved {
            if let Ok(Some(transform)) = state.world.get_mut::<Transform3D>(entity) {
                transform.position = position;
            }
        }
    }

    /// Advances the particle system by `dt` seconds.
    pub fn step_particles(&mut self, delta_seconds: f32) {
        self.state.borrow_mut().particles.step(delta_seconds);
    }

    /// Advances the game clock using real `delta_seconds` and the Lua-configured
    /// rate (`clock.set_rate`). Paused clocks and zero rates do nothing.
    pub fn step_clock(&mut self, delta_seconds: f32) {
        let before = self.state.borrow().game_clock.day;
        let after = {
            let mut state = self.state.borrow_mut();
            state.game_clock.step(delta_seconds);
            state.game_clock.day
        };
        // A clock that crosses midnight while auto-advancing fires `on_day_start`
        // so daily settlement (crops/weather) is not skipped (M16-08).
        if after > before
            && let Ok(function) = self.lua.globals().get::<mlua::Function>("on_day_start")
            && let Err(error) = function.call::<()>(())
        {
            tracing::warn!(target: "script", error = %error, "on_day_start callback failed");
        }
    }

    /// Stores the current viewport so Lua `ui.grid_hit` can resolve screen
    /// coordinates. The host calls this before rendering each frame.
    pub fn set_ui_viewport(&mut self, width: u32, height: u32) {
        self.state.borrow_mut().ui_viewport = [width.max(1) as f32, height.max(1) as f32];
    }

    /// Stores the Canvas 空间 UI 参考分辨率（设计分辨率），供 Lua 侧
    /// `process_ui_clicks` / `ui.grid_hit` 命中测试与渲染保持一致。
    pub fn set_ui_reference(&mut self, reference: [f32; 2]) {
        self.state.borrow_mut().ui_reference = reference;
    }

    /// Sets the directory audio clips are loaded from.
    pub fn set_audio_asset_dir(&mut self, dir: PathBuf) {
        self.state.borrow_mut().audio.set_asset_dir(dir);
    }

    /// Sets the project root, used to resolve `scenes/<name>.scene` on load.
    pub fn set_project_root(&mut self, root: PathBuf) {
        self.state.borrow_mut().project_root = Some(root);
        // 换工程 → 字体也可能换：丢缓存，下次 `ui.measure_text` 重新读盘。
        self.state.borrow_mut().font_probe = None;
    }

    /// Sets the file path save data is committed to / loaded from.
    pub fn set_save_path(&mut self, path: PathBuf) {
        self.state.borrow_mut().save_path = Some(path);
    }

    /// 设置玩家设置根目录（`settings.toml` 的读写根）。
    /// 发布包由宿主指向用户数据目录（见 `core::user_dir`），开发工程不调用，
    /// 于是退回项目根——编辑器与 `jhyt_m20_*` 检查的就是工程内那份。
    pub fn set_settings_root(&mut self, root: PathBuf) {
        self.state.borrow_mut().settings_root = Some(root);
    }

    /// Applies a pending `scene.load(name)` requested by Lua: clears the ECS
    /// world, physics bodies, particles and UI, instantiates the new scene,
    /// and re-runs the project script's `on_create` so the new level is
    /// initialized with fresh entity handles. Saves and audio persist.
    pub fn process_scene_transition(&mut self) -> Result<(), LuaBridgeError> {
        let pending = self.state.borrow().scene_pending_load.clone();
        let Some(name) = pending else {
            return Ok(());
        };
        let root = self
            .state
            .borrow()
            .project_root
            .clone()
            .ok_or_else(|| LuaBridgeError::Api("no project root configured".to_owned()))?;
        let path = root.join("scenes").join(format!("{name}.scene"));
        let scene = Scene::load(&path).map_err(|error| {
            LuaBridgeError::Api(format!("scene {name} failed to load: {error}"))
        })?;
        {
            let mut state = self.state.borrow_mut();
            state.world = World::new();
            state.physics = PhysicsWorld::new();
            state.physics3d = PhysicsWorld3d::new();
            state.particles = ParticleSystem::new();
            state.names.clear();
            state.scene_entities.clear();
            state.persist.clear();
            state.health.clear();
            state.ui.clear();
            state.ui_bars.clear();
            state.ui_panels.clear();
            state.ui_images.clear();
            state.ui_grids.clear();
            state.ui_drag = None;
            state.focus_rings.clear();
            state.scroll_states.clear();
            state.tags.clear();
            state.interact_prompt = None;
            state.schema_defs.clear();
            state.shops.clear();
            state.scene_pending_load = None;
            state.scene = Some(name.clone());
            state.active_camera = None;
            state.active_light = None;
        }
        let instantiated = self.instantiate_scene(&scene)?;
        // 恢复持久实体：新场景未定义的名字用持久快照重生（新场景定义胜出）。
        {
            let mut state = self.state.borrow_mut();
            let persisted: Vec<PersistedEntity> = state.persistent.values().cloned().collect();
            for entity in persisted {
                if state.names.contains_key(&entity.name) {
                    continue;
                }
                Self::spawn_entity_components(
                    &mut state,
                    &entity.name,
                    &entity.components,
                    &entity.resources,
                )
                .map_err(|error| {
                    LuaBridgeError::Api(format!(
                        "restore persistent entity {}: {error}",
                        entity.name
                    ))
                })?;
            }
        }
        call_optional(&self.lua, "on_create").map_err(|error| LuaBridgeError::Runtime {
            path: self
                .current_path
                .clone()
                .unwrap_or_else(|| PathBuf::from("<project script>")),
            source: error.to_string(),
        })?;
        tracing::info!(target: "scene", name = %name, entities = instantiated, "[scene] switched");
        Ok(())
    }

    /// Gives the host a read view of the UI text and bar elements.
    pub fn with_ui<R>(
        &self,
        f: impl FnOnce(&BTreeMap<String, UiText>, &BTreeMap<String, UiBar>) -> R,
    ) -> R {
        let state = self.state.borrow();
        f(&state.ui, &state.ui_bars)
    }

    /// Gives the host a read view of runtime Lua-created UI panels/images/grids.
    pub fn with_dynamic_ui<R>(
        &self,
        f: impl FnOnce(
            &BTreeMap<String, UiPanel>,
            &BTreeMap<String, DynamicUiImage>,
            &BTreeMap<String, UiGrid>,
        ) -> R,
    ) -> R {
        let state = self.state.borrow();
        f(&state.ui_panels, &state.ui_images, &state.ui_grids)
    }

    /// Gives the host a read view of the active particles.
    pub fn with_particles<R>(&self, f: impl FnOnce(&[Particle]) -> R) -> R {
        f(self.state.borrow().particles.particles())
    }

    /// Gives the host a shared read view of the runtime entity world, so the
    /// renderer draws the exact entities the Lua script is mutating.
    pub fn with_world<R>(&self, f: impl FnOnce(&World) -> R) -> R {
        f(&self.state.borrow().world)
    }

    /// Gives the host a shared write view of the runtime entity world.
    pub fn with_world_mut<R>(&mut self, f: impl FnOnce(&mut World) -> R) -> R {
        f(&mut self.state.borrow_mut().world)
    }

    /// Returns the live ECS handle created from an authored scene entity.
    #[must_use]
    pub fn scene_entity_handle(&self, id: SceneEntityId) -> Option<EntityId> {
        self.state.borrow().scene_entities.get(&id).copied()
    }

    /// Returns the authored name registered for an entity, if any. Entities
    /// spawned from a serialized scene keep their scene name; entities spawned
    /// by Lua via `entity.spawn` are registered with the name given at spawn.
    /// Entities never named (or dropped from `names` after despawn) return `None`.
    #[must_use]
    pub fn entity_name(&self, id: EntityId) -> Option<String> {
        self.state
            .borrow()
            .names
            .iter()
            .find(|(_, registered)| **registered == id)
            .map(|(name, _)| name.clone())
    }

    /// Returns the entity whose `Camera3D` the renderer should use, if Lua
    /// selected one with `camera.set_active`.
    #[must_use]
    pub fn active_camera(&self) -> Option<EntityId> {
        self.state.borrow().active_camera
    }

    /// Returns the entity whose `Light` the renderer should use, if Lua
    /// selected one with `light.set_active`.
    #[must_use]
    pub fn active_light(&self) -> Option<EntityId> {
        self.state.borrow().active_light
    }

    pub fn execute_project_script(&mut self, path: &Path) -> Result<(), LuaBridgeError> {
        let source = read_project_script(path)?;
        let candidate = match self.load_candidate(path, &source) {
            Ok(candidate) => candidate,
            Err(error) => {
                self.record_bridge_error(&error);
                return Err(error);
            }
        };
        if let Err(error) = call_optional(&candidate, "on_create") {
            let error = LuaBridgeError::Runtime {
                path: path.to_path_buf(),
                source: error.to_string(),
            };
            self.record_bridge_error(&error);
            return Err(error);
        }
        self.lua = candidate;
        self.current_path = Some(path.to_path_buf());
        self.fingerprint = Some(file_fingerprint(path, &source)?);
        Ok(())
    }

    pub fn reload_if_changed(&mut self) -> Result<ReloadStatus, LuaBridgeError> {
        let path = self
            .current_path
            .clone()
            .ok_or_else(|| LuaBridgeError::Api("no project script is loaded".to_string()))?;
        let source = read_project_script(&path)?;
        let fingerprint = file_fingerprint(&path, &source)?;
        if self.fingerprint.as_ref() == Some(&fingerprint) {
            return Ok(ReloadStatus::Unchanged);
        }
        let candidate = self.load_candidate(&path, &source)?;
        call_optional(&self.lua, "on_destroy").map_err(|error| LuaBridgeError::Runtime {
            path: path.clone(),
            source: error.to_string(),
        })?;
        call_optional(&candidate, "on_create").map_err(|error| LuaBridgeError::Runtime {
            path: path.clone(),
            source: error.to_string(),
        })?;
        self.lua = candidate;
        self.fingerprint = Some(fingerprint);
        Ok(ReloadStatus::Reloaded)
    }

    pub fn reload(&mut self) -> Result<(), LuaBridgeError> {
        let path = self
            .current_path
            .clone()
            .ok_or_else(|| LuaBridgeError::Api("no project script is loaded".to_string()))?;
        let source = read_project_script(&path)?;
        let candidate = self.load_candidate(&path, &source)?;
        call_optional(&self.lua, "on_destroy").map_err(|error| LuaBridgeError::Runtime {
            path: path.clone(),
            source: error.to_string(),
        })?;
        call_optional(&candidate, "on_create").map_err(|error| LuaBridgeError::Runtime {
            path: path.clone(),
            source: error.to_string(),
        })?;
        self.lua = candidate;
        self.fingerprint = Some(file_fingerprint(&path, &source)?);
        Ok(())
    }

    fn load_candidate(&self, path: &Path, source: &str) -> Result<Lua, LuaBridgeError> {
        let candidate = Lua::new();
        register_api(&candidate, Rc::clone(&self.state))
            .map_err(|error| LuaBridgeError::Init(error.to_string()))?;
        // Let Lua `require` resolve sibling modules relative to the project
        // script directory (e.g. `require("player")` -> `scripts/player.lua`).
        if let Ok(package) = candidate.globals().get::<Table>("package")
            && let Ok(existing) = package.get::<String>("path")
            && let Some(script_dir) = path.parent()
        {
            let module_path = format!("{};{}", script_dir.join("?.lua").display(), existing);
            let _ = package.set("path", module_path);
        }
        // 拦截 require：支持压缩/加密的模块文件（明文模块同样可用）。
        if let Some(script_dir) = path.parent() {
            let _ = install_encoded_require_searcher(&candidate, script_dir.to_path_buf());
        }
        candidate
            .load(source)
            .set_name("scripts/main.lua")
            .exec()
            .map_err(|error| LuaBridgeError::Load {
                path: path.to_path_buf(),
                source: error.to_string(),
            })?;
        Ok(candidate)
    }
    pub fn call_update(&self) -> Result<String, LuaBridgeError> {
        let path = self
            .current_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("<project script>"));
        let update: Option<Function> = self
            .lua
            .globals()
            .get("update")
            .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        // No script loaded (engine-only diagnostic mode) or the script has no
        // `update` function: the game simply idles instead of erroring out.
        let Some(update) = update else {
            self.state.borrow_mut().last_update_micros = 0;
            return Ok(String::new());
        };
        let started = std::time::Instant::now();
        let result = update.call(()).map_err(|error| LuaBridgeError::Runtime {
            path,
            source: error.to_string(),
        });
        // 失败也要记账：否则"Lua 抛错的那一帧"在性能面板上看起来是零成本。
        self.state.borrow_mut().last_update_micros =
            u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        result
    }

    /// 上一次 `call_update` 内部 Lua 调用的耗时（微秒）。宿主用它填 `lua_ms`：
    /// 只算 Lua 体，不含 ECS/物理/渲染，指标名与含义因此一致。
    #[must_use]
    pub fn last_update_micros(&self) -> u64 {
        self.state.borrow().last_update_micros
    }

    pub fn call_update_debug(&mut self) -> Result<String, LuaBridgeError> {
        match self.call_update() {
            Ok(value) => Ok(value),
            Err(error) => {
                self.record_bridge_error(&error);
                Err(error)
            }
        }
    }

    /// Executes a short expression or statement in the currently loaded Lua VM.
    /// Console failures are routed through the same structured debugger as script errors.
    pub fn execute_console(&mut self, source: &str) -> Result<String, LuaBridgeError> {
        let path = PathBuf::from("<console>");
        let result = self
            .lua
            .load(source)
            .set_name("<console>")
            .eval::<Value>()
            .map(format_lua_value)
            .map_err(|error| LuaBridgeError::Runtime {
                path: path.clone(),
                source: error.to_string(),
            });
        if let Err(error) = &result {
            self.record_bridge_error(error);
        }
        result
    }

    /// Evaluates a Lua expression in the currently loaded VM and converts the
    /// result to JSON (M19-03: the read-only Lua↔JSON bridge used by editor
    /// tooling, e.g. the domain-data panel and the runtime debug panel).
    ///
    /// Contract:
    /// - Never mutates engine state by itself; the expression is responsible
    ///   for its own side effects (callers pass read-only queries).
    /// - The conversion reuses the same value mapping as `save.blob_*`
    ///   (`lua_value_to_json`): nil/bool/int/float/string/table. Unsupported
    ///   Lua types (function/userdata/thread) are reported as an error.
    /// - Any Lua failure (syntax or runtime) becomes [`LuaBridgeError::Runtime`]
    ///   with the original Lua message preserved verbatim, so the editor can
    ///   show the exact `file:line: message` text.
    pub fn eval_json(&self, expression: &str) -> Result<serde_json::Value, LuaBridgeError> {
        let path = PathBuf::from("<eval_json>");
        let value = self
            .lua
            .load(expression)
            .set_name("<eval_json>")
            .eval::<Value>()
            .map_err(|error| LuaBridgeError::Runtime {
                path: path.clone(),
                source: error.to_string(),
            })?;
        lua_value_to_json(value).map_err(|error| LuaBridgeError::Runtime {
            path,
            source: error.to_string(),
        })
    }

    fn record_bridge_error(&mut self, error: &LuaBridgeError) {
        match error {
            LuaBridgeError::Load { path, source } => {
                self.debugger.record(path.clone(), "load", source.clone())
            }
            LuaBridgeError::Runtime { path, source } => {
                self.debugger
                    .record(path.clone(), "runtime", source.clone())
            }
            LuaBridgeError::Api(source) | LuaBridgeError::Init(source) => {
                self.debugger
                    .record(PathBuf::from("<engine>"), "api", source.clone())
            }
        }
    }

    #[must_use]
    pub fn diagnostics(&self) -> Vec<ScriptDiagnostic> {
        self.debugger.visible()
    }

    #[must_use]
    pub fn all_diagnostics(&self) -> &[ScriptDiagnostic] {
        self.debugger.all()
    }

    pub fn clear_diagnostics(&mut self) {
        self.debugger.clear();
    }

    pub fn set_debug_filter(&mut self, filter: Option<String>) {
        self.debugger.set_filter(filter);
    }

    #[must_use]
    pub fn quit_requested(&self) -> bool {
        self.state.borrow().quit_requested
    }
}

fn find_line_number(source: &str) -> Option<u32> {
    let bytes = source.as_bytes();
    for index in 0..bytes.len() {
        if bytes[index] != b':' {
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > start
            && (end == bytes.len() || bytes[end] == 58 || bytes[end] == 32)
            && let Ok(line) = source[start..end].parse()
        {
            return Some(line);
        }
    }
    None
}

fn file_fingerprint(path: &Path, source: &str) -> Result<ScriptFingerprint, LuaBridgeError> {
    use std::hash::{Hash, Hasher};
    let metadata = fs::metadata(path).map_err(|error| LuaBridgeError::Load {
        path: path.to_path_buf(),
        source: error.to_string(),
    })?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    Ok(ScriptFingerprint {
        modified: metadata.modified().ok(),
        length: metadata.len(),
        hash: hasher.finish(),
    })
}

fn format_lua_value(value: Value) -> String {
    match value {
        Value::Nil => "nil".to_owned(),
        Value::Boolean(value) => value.to_string(),
        Value::Integer(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.to_string_lossy(),
        Value::Table(_) => "<table>".to_owned(),
        Value::Function(_) => "<function>".to_owned(),
        Value::Thread(_) => "<thread>".to_owned(),
        Value::UserData(_) => "<userdata>".to_owned(),
        Value::LightUserData(_) => "<lightuserdata>".to_owned(),
        Value::Error(error) => error.to_string(),
        Value::Other(_) => "<other>".to_owned(),
    }
}

fn call_optional(lua: &Lua, name: &str) -> LuaResult<()> {
    let callback: Value = lua.globals().get(name)?;
    if let Value::Function(function) = callback {
        function.call::<()>(())?;
    }
    Ok(())
}
fn api_error(message: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(message.into())
}

/// Calls a global Lua hook (`on_day_end` / `on_day_start`) if the script defines
/// one. Missing hooks are a no-op; a hook that errors propagates the error.
fn call_optional_day_hook(lua: &mlua::Lua, name: &str) -> Result<(), mlua::Error> {
    if let Ok(function) = lua.globals().get::<mlua::Function>(name) {
        function.call::<()>(())?;
    }
    Ok(())
}
fn entity_from_lua(value: i64) -> Result<EntityId, mlua::Error> {
    if value < 0 {
        return Err(api_error("invalid entity id: negative value"));
    }
    let raw = value as u64;
    let index = (raw >> 32) as u32;
    let generation = raw as u32;
    if generation == 0 {
        return Err(api_error(format!("invalid entity id: {value}")));
    }
    Ok(EntityId::from_raw(index, generation))
}
fn entity_to_lua(entity: EntityId) -> i64 {
    ((entity.index() as u64) << 32 | entity.generation() as u64) as i64
}
fn require_alive(state: &ScriptState, entity: EntityId) -> Result<(), mlua::Error> {
    if state.world.is_alive(entity) {
        Ok(())
    } else {
        Err(api_error(format!("invalid entity id: {entity}")))
    }
}
fn table_vec(lua: &Lua, x: f32, y: f32) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set(1, x)?;
    table.set(2, y)?;
    Ok(table)
}

fn table_i32_pair(lua: &Lua, x: i32, y: i32) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set(1, x)?;
    table.set(2, y)?;
    Ok(table)
}

fn require_positive_tile_size(tile_size: f32) -> Result<(), mlua::Error> {
    if !tile_size.is_finite() || tile_size <= 0.0 {
        return Err(api_error("tile_size must be finite and > 0"));
    }
    Ok(())
}

fn finite_ui_rect(x: f32, y: f32, width: f32, height: f32) -> bool {
    x.is_finite()
        && y.is_finite()
        && width.is_finite()
        && height.is_finite()
        && width > 0.0
        && height > 0.0
}

fn finite_color4(color: [f32; 4]) -> bool {
    color.iter().all(|channel| channel.is_finite())
}

/// Resolves the save file path for the active slot. `None` keeps the original
/// path.
fn save_path_for_slot(path: &Path, slot: Option<&str>) -> PathBuf {
    let Some(slot) = slot else {
        return path.to_path_buf();
    };
    let safe: String = slot
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() {
        "default".to_owned()
    } else {
        safe
    };
    path.with_file_name(format!("save_{safe}.sav"))
}

/// Queues a scene load when the scene file exists; mirrors `scene.load` so
/// `map.goto` can share the same validation without duplicating it.
fn request_scene_load(state: &ScriptState, name: &str) -> bool {
    let exists = state
        .project_root
        .as_ref()
        .is_some_and(|root| root.join("scenes").join(format!("{name}.scene")).is_file());
    if exists {
        tracing::info!(target: "scene", name = %name, "[map] scene transition queued");
    } else {
        tracing::warn!(target: "scene", name = %name, "[map] scene file not found; load ignored");
    }
    exists
}

/// Converts an arbitrary Lua value into JSON so `save.blob_*` and map state
/// can round-trip through the existing string-keyed save file.
fn lua_value_to_json(value: Value) -> Result<serde_json::Value, mlua::Error> {
    match value {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Boolean(value) => Ok(serde_json::Value::Bool(value)),
        Value::Integer(value) => Ok(serde_json::Value::Number(value.into())),
        Value::Number(value) => {
            if !value.is_finite() {
                return Err(api_error("blob value must contain finite numbers"));
            }
            serde_json::Number::from_f64(value)
                .map(serde_json::Value::Number)
                .ok_or_else(|| api_error("blob value contains an invalid number"))
        }
        Value::String(value) => value
            .to_str()
            .map(|text| serde_json::Value::String(text.to_owned()))
            .map_err(mlua::Error::external),
        Value::Table(table) => lua_table_to_json(&table),
        _ => Err(api_error("unsupported Lua value type for blob storage")),
    }
}

fn lua_table_to_json(table: &Table) -> Result<serde_json::Value, mlua::Error> {
    let len = table.raw_len();
    let mut array = vec![serde_json::Value::Null; len];
    let mut array_like = true;
    for pair in table.clone().pairs() {
        let (key, value) = pair?;
        let converted = lua_value_to_json(value)?;
        match key {
            Value::Integer(index) if index >= 1 && (index as usize) <= len => {
                array[(index - 1) as usize] = converted;
            }
            _ => {
                array_like = false;
                break;
            }
        }
    }
    if array_like && len > 0 {
        return Ok(serde_json::Value::Array(array));
    }

    let mut object = serde_json::Map::new();
    for pair in table.clone().pairs() {
        let (key, value) = pair?;
        let key = match key {
            Value::Integer(index) => index.to_string(),
            Value::String(text) => text.to_str()?.to_owned(),
            _ => return Err(api_error("blob table keys must be strings or integers")),
        };
        object.insert(key, lua_value_to_json(value)?);
    }
    Ok(serde_json::Value::Object(object))
}

fn json_to_lua_value(lua: &Lua, value: &serde_json::Value) -> LuaResult<Value> {
    match value {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(value) => Ok(Value::Boolean(*value)),
        serde_json::Value::Number(value) => {
            if let Some(integer) = value.as_i64() {
                Ok(Value::Integer(integer))
            } else {
                Ok(Value::Number(value.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(value) => Ok(Value::String(lua.create_string(value)?)),
        serde_json::Value::Array(values) => {
            let table = lua.create_table()?;
            for (index, value) in values.iter().enumerate() {
                table.set(index + 1, json_to_lua_value(lua, value)?)?;
            }
            Ok(Value::Table(table))
        }
        serde_json::Value::Object(values) => {
            let table = lua.create_table()?;
            for (key, value) in values {
                table.set(key.as_str(), json_to_lua_value(lua, value)?)?;
            }
            Ok(Value::Table(table))
        }
    }
}

fn inventory_index(index: i64) -> Result<usize, mlua::Error> {
    if index <= 0 {
        Err(api_error("inventory slot index must be positive"))
    } else {
        Ok((index - 1) as usize)
    }
}

fn inventory_clone(state: &ScriptState, id: &str) -> Result<Inventory, mlua::Error> {
    state
        .inventories
        .get(id)
        .cloned()
        .ok_or_else(|| api_error(format!("inventory {id} not found")))
}

fn swap_inventories(
    state: &mut ScriptState,
    from_id: &str,
    from_index: i64,
    to_id: &str,
    to_index: i64,
) -> Result<bool, mlua::Error> {
    let from_index = inventory_index(from_index)?;
    let to_index = inventory_index(to_index)?;
    if from_id == to_id {
        let mut inventory = inventory_clone(state, from_id)?;
        if from_index >= inventory.capacity || to_index >= inventory.capacity {
            return Err(api_error("inventory slot out of bounds"));
        }
        inventory.slots.swap(from_index, to_index);
        state.inventories.insert(from_id.to_owned(), inventory);
        return Ok(true);
    }
    let mut from = inventory_clone(state, from_id)?;
    let mut to = inventory_clone(state, to_id)?;
    if from_index >= from.capacity || to_index >= to.capacity {
        return Err(api_error("inventory slot out of bounds"));
    }
    let from_slot = from.slots[from_index].clone();
    let to_slot = to.slots[to_index].clone();
    from.slots[from_index] = to_slot;
    to.slots[to_index] = from_slot;
    state.inventories.insert(from_id.to_owned(), from);
    state.inventories.insert(to_id.to_owned(), to);
    Ok(true)
}

fn move_inventories(
    state: &mut ScriptState,
    from_id: &str,
    from_index: i64,
    to_id: &str,
    to_index: Option<i64>,
    count: Option<i64>,
) -> Result<i64, mlua::Error> {
    let from_index = inventory_index(from_index)?;
    let to_index = to_index.map(inventory_index).transpose()?;
    if from_id == to_id {
        let mut inventory = inventory_clone(state, from_id)?;
        if from_index >= inventory.capacity {
            return Err(api_error(format!("inventory {from_id} slot out of bounds")));
        }
        let source = inventory.slots[from_index].clone();
        if source.item_id.is_empty() {
            return Ok(0);
        }
        let max_stack = state
            .item_defs
            .get(&source.item_id)
            .map_or(99, |definition| definition.stack);
        if let Some(to_index) = to_index {
            if to_index >= inventory.capacity {
                return Err(api_error(format!("inventory {to_id} slot out of bounds")));
            }
            let target = inventory.slots[to_index].clone();
            let requested = count
                .unwrap_or(i64::from(source.count))
                .min(i64::from(source.count))
                .max(0) as u32;
            let moved = if target.item_id == source.item_id {
                max_stack.saturating_sub(target.count).min(requested)
            } else if target.item_id.is_empty() {
                requested
            } else {
                0
            };
            if moved > 0 {
                inventory.slots[from_index].count -= moved;
                if inventory.slots[from_index].count == 0 {
                    inventory.slots[from_index] = ItemStack::default();
                }
                if target.item_id == source.item_id {
                    inventory.slots[to_index].count += moved;
                } else {
                    inventory.slots[to_index] = ItemStack::new(source.item_id.clone(), moved);
                }
            }
            state.inventories.insert(from_id.to_owned(), inventory);
            return Ok(moved as i64);
        }
        let requested = count.unwrap_or(i64::from(source.count)).max(1) as u32;
        let removed = inventory.remove(&source.item_id, requested);
        let leftover = inventory.add(&source.item_id, removed, max_stack);
        state.inventories.insert(from_id.to_owned(), inventory);
        return Ok(removed as i64 - leftover as i64);
    }
    let mut from = inventory_clone(state, from_id)?;
    let mut to = inventory_clone(state, to_id)?;
    if from_index >= from.capacity {
        return Err(api_error(format!("inventory {from_id} slot out of bounds")));
    }
    let source = from.slots[from_index].clone();
    if source.item_id.is_empty() {
        return Ok(0);
    }
    let max_stack = state
        .item_defs
        .get(&source.item_id)
        .map_or(99, |definition| definition.stack);
    if let Some(to_index) = to_index {
        if to_index >= to.capacity {
            return Err(api_error(format!("inventory {to_id} slot out of bounds")));
        }
        let target = to.slots[to_index].clone();
        let requested = count
            .unwrap_or(i64::from(source.count))
            .min(i64::from(source.count))
            .max(0) as u32;
        let moved = if target.item_id == source.item_id {
            max_stack.saturating_sub(target.count).min(requested)
        } else if target.item_id.is_empty() {
            requested
        } else {
            0
        };
        if moved > 0 {
            from.slots[from_index].count -= moved;
            if from.slots[from_index].count == 0 {
                from.slots[from_index] = ItemStack::default();
            }
            if target.item_id == source.item_id {
                to.slots[to_index].count += moved;
            } else {
                to.slots[to_index] = ItemStack::new(source.item_id.clone(), moved);
            }
        }
        state.inventories.insert(from_id.to_owned(), from);
        state.inventories.insert(to_id.to_owned(), to);
        return Ok(moved as i64);
    }
    let requested = count.unwrap_or(i64::from(source.count)).max(1) as u32;
    let removed = from.remove(&source.item_id, requested);
    let leftover = to.add(&source.item_id, removed, max_stack);
    if leftover > 0 {
        let _ = from.add(&source.item_id, leftover, max_stack);
    }
    state.inventories.insert(from_id.to_owned(), from);
    state.inventories.insert(to_id.to_owned(), to);
    Ok(removed as i64 - leftover as i64)
}

/// 读取项目脚本源码，支持发布包的压缩/加密（明文原样返回）。
fn read_project_script(path: &Path) -> Result<String, LuaBridgeError> {
    let bytes = fs::read(path).map_err(|error| LuaBridgeError::Load {
        path: path.to_path_buf(),
        source: error.to_string(),
    })?;
    let decoded = crate::core::pack::decode(&bytes).map_err(|error| LuaBridgeError::Load {
        path: path.to_path_buf(),
        source: error,
    })?;
    Ok(String::from_utf8_lossy(&decoded).into_owned())
}

/// 在 Lua VM 上安装支持压缩/加密的 require 搜索器：
/// 读取模块文件字节 → 解码 → 编译，供 `require` 加载发布包中的模块。
fn install_encoded_require_searcher(lua: &Lua, script_dir: PathBuf) -> mlua::Result<()> {
    let searcher = lua.create_function(move |lua, module: String| {
        let relative = module.replace('.', std::path::MAIN_SEPARATOR_STR);
        let file_path = script_dir.join(format!("{relative}.lua"));
        if !file_path.is_file() {
            return Ok(Value::Nil);
        }
        let bytes = fs::read(&file_path).map_err(mlua::Error::external)?;
        let decoded = crate::core::pack::decode(&bytes).map_err(mlua::Error::external)?;
        let source = String::from_utf8_lossy(&decoded).into_owned();
        let chunk_name = format!("scripts/{module}.lua");
        let chunk = lua.load(&source).set_name(chunk_name).into_function()?;
        Ok(Value::Function(chunk))
    })?;
    let package: Table = lua.globals().get("package")?;
    let searchers: Table = package.get("searchers")?;
    searchers.raw_insert(1, searcher)?;
    Ok(())
}

/// Writes `text` to `path` atomically (temp file + rename), like scene saves.
fn atomic_write(path: &Path, text: &str) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(temp, path)?;
    Ok(())
}

/// Converts a tile map's solid cells into static cuboid colliders attached to
/// the tile map entity's fixed body, one per solid cell.
fn solid_cell_colliders(tilemap: &Tilemap) -> Vec<ColliderDesc> {
    let half = Vec2::new(tilemap.tile_size.x / 2.0, tilemap.tile_size.y / 2.0);
    let mut colliders = Vec::new();
    for y in 0..tilemap.height {
        for x in 0..tilemap.width {
            if tilemap.solid[y * tilemap.width + x] {
                colliders.push(ColliderDesc {
                    shape: ColliderShape::Cuboid(half),
                    offset: Vec2::new(
                        x as f32 * tilemap.tile_size.x + half.x,
                        y as f32 * tilemap.tile_size.y + half.y,
                    ),
                    sensor: false,
                    layer: 0xFF,
                    mask: 0xFF,
                });
            }
        }
    }
    colliders
}

/// Rebuilds the fixed physics body for a tile map after Lua changes its solid
/// layer, so runtime `tilemap.set_solid` immediately affects movement.
fn refresh_tilemap_physics(state: &mut ScriptState, entity: EntityId) {
    let position = state
        .world
        .get::<Transform>(entity)
        .ok()
        .flatten()
        .map(|transform| transform.position)
        .unwrap_or_default();
    let _ = state.physics.remove_body(entity);
    let Some(tilemap) = state.world.get::<Tilemap>(entity).ok().flatten() else {
        return;
    };
    if tilemap.solid.iter().any(|solid| *solid) {
        let colliders = solid_cell_colliders(tilemap);
        let _ = state.physics.add_body(entity, position, false, colliders);
    }
}

fn register_api(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let log = lua.create_table()?;
    log.set(
        "info",
        lua.create_function(|_, message: String| {
            info!(target: "script", message = %message, "Lua log.info");
            Ok(())
        })?,
    )?;
    log.set(
        "warn",
        lua.create_function(|_, message: String| {
            warn!(target: "script", message = %message, "Lua log.warn");
            Ok(())
        })?,
    )?;
    log.set(
        "error",
        lua.create_function(|_, message: String| {
            error!(target: "script", message = %message, "Lua log.error");
            Ok(())
        })?,
    )?;
    lua.globals().set("log", log)?;

    let time = lua.create_table()?;
    let s = Rc::clone(&state);
    time.set(
        "delta",
        lua.create_function(move |_, ()| Ok(s.borrow().delta_seconds))?,
    )?;
    lua.globals().set("time", time)?;

    let clock = lua.create_table()?;
    let s = Rc::clone(&state);
    clock.set(
        "now",
        lua.create_function(move |lua, ()| {
            let clock = &s.borrow().game_clock;
            let now = lua.create_table()?;
            now.set("day", clock.day as i64)?;
            now.set("minute", clock.minute as i64)?;
            now.set("hour", (clock.minute / 60) as i64)?;
            now.set("minute_of_day", clock.minute as i64)?;
            now.set("rate", clock.rate)?;
            now.set("paused", clock.paused)?;
            Ok(now)
        })?,
    )?;
    let s = Rc::clone(&state);
    clock.set(
        "set_rate",
        lua.create_function(move |_, rate: f32| {
            if !rate.is_finite() || rate < 0.0 {
                return Err(api_error("clock.set_rate must be finite and non-negative"));
            }
            let mut state = s.borrow_mut();
            state.game_clock.rate = rate;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    clock.set(
        "pause",
        lua.create_function(move |_, paused: bool| {
            s.borrow_mut().game_clock.paused = paused;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    clock.set(
        "add",
        lua.create_function(move |_, minutes: f32| {
            if !minutes.is_finite() {
                return Err(api_error("clock.add minutes must be finite"));
            }
            s.borrow_mut().game_clock.add_minutes(minutes);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    clock.set(
        "set",
        lua.create_function(move |_, (day, minute): (i64, i64)| {
            if day < 1 || minute < 0 {
                return Err(api_error("clock.set day must be >= 1 and minute >= 0"));
            }
            let mut state = s.borrow_mut();
            state.game_clock.set(
                day as u32,
                minute.min((crate::core::gameplay::MINUTES_PER_DAY - 1) as i64) as u32,
            );
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    clock.set(
        "sleep_to_morning",
        lua.create_function(move |_, ()| {
            let day = s.borrow_mut().game_clock.sleep_to_morning();
            Ok(day as i64)
        })?,
    )?;
    lua.globals().set("clock", clock)?;

    // `calendar` derives season/year from the absolute in-game day (M16-08).
    let calendar = lua.create_table()?;
    let s = Rc::clone(&state);
    calendar.set(
        "now",
        lua.create_function(move |lua, ()| {
            let clock = &s.borrow().game_clock;
            let now = lua.create_table()?;
            now.set("absolute_day", clock.day as i64)?;
            now.set("day", clock.day_of_season() as i64)?;
            now.set("season", clock.season() as i64)?;
            now.set("year", clock.year() as i64)?;
            now.set("minute", clock.minute as i64)?;
            Ok(now)
        })?,
    )?;
    let s = Rc::clone(&state);
    calendar.set(
        "set",
        lua.create_function(
            move |_, (day, season, year, minute): (i64, i64, i64, i64)| {
                let days = crate::core::gameplay::DAYS_PER_SEASON as i64;
                if day < 1 || day > days {
                    return Err(api_error(format!("calendar.set day must be in 1..={days}")));
                }
                if !(1..=crate::core::gameplay::SEASONS_PER_YEAR as i64).contains(&season) {
                    return Err(api_error("calendar.set season must be in 1..=4"));
                }
                if year < 1 {
                    return Err(api_error("calendar.set year must be >= 1"));
                }
                let absolute = (year - 1) * crate::core::gameplay::DAYS_PER_YEAR as i64
                    + (season - 1) * days
                    + day;
                let mut state = s.borrow_mut();
                state
                    .game_clock
                    .set_absolute(absolute as u32, minute.max(0) as u32);
                Ok(())
            },
        )?,
    )?;
    lua.globals().set("calendar", calendar)?;

    // `day` lifecycle: sleep to the next morning, firing `on_day_end` then
    // `on_day_start` (M16-08). Gameplay registers those globals for settlement.
    let day = lua.create_table()?;
    let s = Rc::clone(&state);
    day.set(
        "sleep",
        lua.create_function(move |lua, ()| {
            call_optional_day_hook(lua, "on_day_end")?;
            let new_day = {
                let mut state = s.borrow_mut();
                state.game_clock.sleep_to_morning()
            };
            call_optional_day_hook(lua, "on_day_start")?;
            Ok(new_day as i64)
        })?,
    )?;
    let s = Rc::clone(&state);
    day.set(
        "now",
        lua.create_function(move |lua, ()| {
            let clock = &s.borrow().game_clock;
            let now = lua.create_table()?;
            now.set("day", clock.day as i64)?;
            now.set("minute", clock.minute as i64)?;
            Ok(now)
        })?,
    )?;
    lua.globals().set("day", day)?;

    let input = lua.create_table()?;
    for (name, mode) in [("is_down", 0_u8), ("just_pressed", 1), ("just_released", 2)] {
        let s = Rc::clone(&state);
        input.set(
            name,
            lua.create_function(move |_, key: String| {
                // M22/W1：键名解析失败是**错误**（未知名不再静默落 Other(0)）。
                let key = parse_key_strict(&key)?;
                let state = s.borrow();
                Ok(match mode {
                    0 => state.input.is_down(key),
                    1 => state.input.just_pressed(key),
                    _ => state.input.just_released(key),
                })
            })?,
        )?;
    }
    let s = Rc::clone(&state);
    input.set(
        "pressed",
        lua.create_function(move |_, key: String| {
            let key = parse_key_strict(&key)?;
            Ok(s.borrow().input.just_pressed(key))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "released",
        lua.create_function(move |_, key: String| {
            let key = parse_key_strict(&key)?;
            Ok(s.borrow().input.just_released(key))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "axis",
        lua.create_function(move |_, axis: String| Ok(s.borrow().input.axis(&axis)))?,
    )?;
    for (name, mode) in [
        ("mouse_is_down", 0_u8),
        ("mouse_just_pressed", 1),
        ("mouse_just_released", 2),
    ] {
        let s = Rc::clone(&state);
        input.set(
            name,
            lua.create_function(move |_, button: String| {
                let input = &s.borrow().input;
                let button = parse_mouse_button(&button);
                Ok(match mode {
                    0 => input.mouse_is_down(button),
                    1 => input.mouse_just_pressed(button),
                    _ => input.mouse_just_released(button),
                })
            })?,
        )?;
    }
    let s = Rc::clone(&state);
    input.set(
        "cursor_position",
        lua.create_function(move |lua, ()| {
            let p = s.borrow().input.cursor_position();
            table_vec(lua, p.0 as f32, p.1 as f32)
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "cursor_delta",
        lua.create_function(move |lua, ()| {
            let p = s.borrow().input.cursor_delta();
            table_vec(lua, p.0 as f32, p.1 as f32)
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "scroll_delta",
        lua.create_function(move |lua, ()| {
            let p = s.borrow().input.scroll_delta();
            table_vec(lua, p.0, p.1)
        })?,
    )?;
    // ---- M20-07：命名动作层 + [input] 设置持久化 ---------------------------
    // 三层模型：物理输入（键鼠 + 宿主喂入的手柄设备帧）→ 命名动作
    // （`ScriptState::action_map`，一份实例）→ 动作消费（下面的 `input.action*`）。
    // 旧物理键 API（`is_down`/`just_pressed`/`just_released`/`axis`/`pressed`/
    // `released`）语义**不变**并继续可用，老脚本与 headless 用例不受影响；
    // 只有 `input.action` 从「物理键别名」升级为「命名动作查询」（全仓库除本
    // 文件外无调用方，见 M20-07 文档的迁移说明）。
    let s = Rc::clone(&state);
    input.set(
        "action",
        lua.create_function(move |_, name: String| {
            let state = s.borrow();
            Ok(state.input.action_is_down(&state.action_map, &name))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "action_pressed",
        lua.create_function(move |_, name: String| {
            let state = s.borrow();
            Ok(state.input.action_just_pressed(&state.action_map, &name))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "action_released",
        lua.create_function(move |_, name: String| {
            let state = s.borrow();
            Ok(state.input.action_just_released(&state.action_map, &name))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "action_axis",
        lua.create_function(move |_, (negative, positive): (String, String)| {
            let state = s.borrow();
            Ok(state
                .input
                .action_axis(&state.action_map, &negative, &positive))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "pad_button",
        lua.create_function(move |_, index: i64| {
            Ok(s.borrow().input.gamepad_button(index.clamp(0, 255) as u8))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "pad_axis",
        lua.create_function(move |_, index: i64| {
            Ok(s.borrow().input.gamepad_axis(index.clamp(0, 255) as u8))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "actions",
        lua.create_function(move |lua, ()| {
            let state = s.borrow();
            let table = lua.create_table()?;
            for (index, name) in state.action_map.actions().into_iter().enumerate() {
                table.set(index + 1, name)?;
            }
            Ok(table)
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "bindings",
        lua.create_function(move |lua, name: String| {
            let state = s.borrow();
            let table = lua.create_table()?;
            for (index, binding) in state.action_map.bindings(&name).iter().enumerate() {
                table.set(index + 1, binding.to_text())?;
            }
            Ok(table)
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "rebind",
        lua.create_function(move |_, (name, binding): (String, String)| {
            let parsed = match Binding::parse(binding.trim()) {
                Ok(parsed) => parsed,
                Err(error) => return Ok((false, error.to_string())),
            };
            let mut state = s.borrow_mut();
            // 重绑定 = 覆盖该动作的全部旧绑定（单一主绑定语义，与面板 UI 一致）；
            // 需要一键多绑定时改设置文件的 `[input].map` 文本。
            state.action_map.clear(&name);
            match state.action_map.bind(&name, parsed) {
                Ok(()) => Ok((true, String::new())),
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "unbind",
        lua.create_function(move |_, (name, binding): (String, String)| {
            let Ok(parsed) = Binding::parse(binding.trim()) else {
                return Ok(false);
            };
            Ok(s.borrow_mut().action_map.unbind(&name, parsed))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "reset_bindings",
        lua.create_function(move |_, ()| {
            // 唯一重置入口：绑定是玩家偏好，不随场景切换/读档回滚（ADR-0021）。
            s.borrow_mut().action_map = ActionMap::with_defaults();
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "action_map_text",
        lua.create_function(move |_, ()| Ok(s.borrow().action_map.to_text()))?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "set_action_map",
        lua.create_function(move |_, text: String| {
            // M22/W1：接受 v1 与 v2 两种版本头，v1 的键位会被迁移成物理键，并把
            // 有损换算记进 `input.migration_notes()`。
            match ActionMap::from_text_with_notes(&text) {
                Ok((map, notes)) => {
                    let mut state = s.borrow_mut();
                    state.action_map = map;
                    state.input_migration_notes = notes;
                    Ok((true, String::new()))
                }
                // 版本头不符 → false + 原因，并且**不改动**当前映射（不静默回退）。
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "diagnostics",
        lua.create_function(move |lua, ()| {
            let state = s.borrow();
            let table = lua.create_table()?;
            for (index, message) in state.input_diagnostics.iter().enumerate() {
                table.set(index + 1, message.clone())?;
            }
            Ok(table)
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "migration_notes",
        // M22/W1：v1 → v2 改键迁移说明（有损换算逐条）。与 `diagnostics()` 分开：
        // 诊断 = 「出错了」，迁移说明 = 「变了但没丢」。启动时打印一次即可。
        lua.create_function(move |lua, ()| {
            let state = s.borrow();
            let table = lua.create_table()?;
            for (index, message) in state.input_migration_notes.iter().enumerate() {
                table.set(index + 1, message.clone())?;
            }
            Ok(table)
        })?,
    )?;
    // 设置文件 `settings.toml` 的 `[input]` 段（M20-07）。路径规则与
    // `audio.settings_path/load_settings/save_settings` 一致：缺省的根由
    // `settings_root()` 决定（P3：发布包 → 用户数据目录，开发工程 → 项目根），
    // 给相对路径时相对该根解析（无根则要求绝对路径）。
    let s = Rc::clone(&state);
    input.set(
        "settings_path",
        lua.create_function(move |_, ()| {
            let root = settings_root(&s.borrow());
            Ok(root.map(|root| {
                ProjectSettings::path_in(&root)
                    .to_string_lossy()
                    .into_owned()
            }))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "load_settings",
        lua.create_function(move |_, path: Option<String>| {
            let resolved = resolve_input_settings_path(&s.borrow(), path.as_deref());
            let Some(path) = resolved else {
                return Ok((false, "no_project_root".to_owned()));
            };
            let existing = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // 文件不存在：保留当前绑定（不是错误），返回 false + "missing"
                    //（与 `audio.load_settings` 的既有语义一致）。
                    return Ok((false, "missing".to_owned()));
                }
                Err(error) => {
                    let message = format!("{}: read failed: {error}", path.display());
                    push_input_diagnostic(&mut s.borrow_mut(), message);
                    return Ok((false, error.to_string()));
                }
            };
            let parsed = crate::core::settings::parse_input_section(&existing);
            // M22/W3：读改键的同时把同一个 `[input]` 段的**导航手感**应用到引擎
            //（阈值 + 重复节奏），这样游戏只需 `input.load_settings()` 一次调用即可，
            // 不必再单独调 `apply_nav_settings()`。文件里没有这些键时就是默认手感。
            s.borrow_mut().nav = crate::core::settings::parse_nav_section(&existing);
            // M22/W1：v1 → v2 迁移说明（有损换算逐条），供 `input.migration_notes()`。
            s.borrow_mut().input_migration_notes = parsed.migration_notes.clone();
            match parsed.map {
                Some(mut map) => {
                    // 文件里没有的动作用默认绑定补齐（新增动作在旧设置文件上仍可玩），
                    // 用户重绑定不被覆盖（`merge_missing` 只补缺失动作）。
                    map.merge_missing(&ActionMap::with_defaults());
                    s.borrow_mut().action_map = map;
                    Ok((true, "loaded".to_owned()))
                }
                None => {
                    let reason = parsed
                        .reason
                        .unwrap_or_else(|| "invalid_input_section".to_owned());
                    let missing_key = reason.starts_with("missing_key");
                    if !missing_key {
                        // 内容非法（版本头不符 / 绑定文本非法）才是「回退默认」，
                        // 并留下可定位诊断；缺键只诊断、在位的绑定保持不变。
                        s.borrow_mut().action_map = ActionMap::with_defaults();
                    }
                    let mut state = s.borrow_mut();
                    let message = if missing_key {
                        format!("{}: {reason}; keeping current bindings", path.display())
                    } else {
                        format!("{}: {reason}; reverting to defaults", path.display())
                    };
                    push_input_diagnostic(&mut state, message.clone());
                    Ok((false, message))
                }
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "save_settings",
        lua.create_function(move |_, path: Option<String>| {
            let (resolved, map) = {
                let state = s.borrow();
                (
                    resolve_input_settings_path(&state, path.as_deref()),
                    state.action_map.clone(),
                )
            };
            let Some(path) = resolved else {
                return Ok((false, "no_project_root".to_owned()));
            };
            // 保序读改写：只动 `[input]` 段，`[audio]` 段与注释原样保留。
            let existing = fs::read_to_string(&path).unwrap_or_default();
            if let Some(parent) = path.parent()
                && let Err(error) = fs::create_dir_all(parent)
            {
                return Ok((false, error.to_string()));
            }
            match atomic_write(
                &path,
                &crate::core::settings::upsert_input_section(&existing, &map),
            ) {
                Ok(()) => Ok((true, "saved".to_owned())),
                Err(error) => {
                    let message = format!("{}: save failed: {error}", path.display());
                    push_input_diagnostic(&mut s.borrow_mut(), message);
                    Ok((false, error.to_string()))
                }
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "nav_settings",
        lua.create_function(move |lua, path: Option<String>| {
            // M20-09 / M22-W3：菜单导航手感（摇杆阈值 + 按住重复节奏）与
            // `load_settings` 同源——同一个 `[input]` 段、同一套宽松解析与钳制。
            // 缺省路径 = 工程根。重复节奏自 M22/W3 起是**毫秒**（旧的帧键仍可读，
            // 按 60Hz 换算）。
            let resolved = resolve_input_settings_path(&s.borrow(), path.as_deref());
            let Some(path) = resolved else {
                return Ok(None);
            };
            let nav = match fs::read_to_string(&path) {
                Ok(text) => crate::core::settings::parse_nav_section(&text),
                // 文件缺失/不可读 → 默认手感（首次运行的正常路径）。
                Err(_) => NavSettings::DEFAULT,
            };
            let table = lua.create_table()?;
            table.set("threshold", nav.threshold)?;
            table.set("delay_ms", nav.delay_ms)?;
            table.set("repeat_ms", nav.repeat_ms)?;
            Ok(Some(table))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "set_nav_settings",
        // M22/W3：把导航手感推给引擎（阈值同时作为设备层轴判定阈值，重复节奏供
        // `input.action_repeat` 使用）。三个参数都按 `NavSettings::new` 钳制。
        lua.create_function(
            move |_, (threshold, delay_ms, repeat_ms): (f32, u32, u32)| {
                let nav = NavSettings::new(threshold, delay_ms, repeat_ms);
                s.borrow_mut().nav = nav;
                Ok((nav.threshold, nav.delay_ms, nav.repeat_ms))
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "action_repeat",
        // M22/W3：菜单导航的「首次立即 + 按住连发」。时间制（毫秒），因此键盘、DPad、
        // 摇杆在不同帧率下手感一致；只有模拟量绑定会连发（数字键保持一次一步）。
        lua.create_function(move |_, name: String| Ok(s.borrow_mut().action_repeat(&name)))?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "last_device",
        // M22/W3：最近一次产生真实输入的设备类别（"keyboard" / "gamepad"）。
        // 引擎在边沿发生时记录，因此 Lua 侧不再需要每帧扫描全部绑定来猜设备。
        lua.create_function(move |_, ()| Ok(s.borrow().input.last_device().to_text()))?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "rumble",
        // M22/W2：请求震动（左右马达 0..1，毫秒）。这里只登记请求，宿主在帧末交给设备层
        //（`GamepadPoller::rumble` 负责时长上限与到期归零）；无手柄时静默无效。
        lua.create_function(move |_, (low, high, ms): (f32, f32, u32)| {
            s.borrow_mut().rumble_request = Some((low, high, ms));
            Ok(())
        })?,
    )?;
    lua.globals().set("input", input)?;

    let entity = lua.create_table()?;
    let s = Rc::clone(&state);
    entity.set(
        "spawn",
        lua.create_function(
            move |_, (name, persist_id): (Option<String>, Option<String>)| {
                let mut state = s.borrow_mut();
                let id = state.world.spawn();
                state
                    .world
                    .insert(id, Transform::default())
                    .map_err(world_error)?;
                if let Some(name) = name {
                    state.names.insert(name, id);
                }
                if let Some(persist) = persist_id.filter(|value| !value.is_empty()) {
                    if state
                        .persist
                        .get(&persist)
                        .is_some_and(|existing| state.world.is_alive(*existing))
                    {
                        return Err(api_error(format!("persist_id already in use: {persist}")));
                    }
                    state.persist.insert(persist, id);
                }
                Ok(entity_to_lua(id))
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "find",
        lua.create_function(move |_, name: String| {
            Ok(s.borrow().names.get(&name).copied().map(entity_to_lua))
        })?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "find_id",
        lua.create_function(move |_, persist_id: String| {
            Ok(s.borrow()
                .persist
                .get(&persist_id)
                .copied()
                .map(entity_to_lua))
        })?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "persist_id",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            Ok(s.borrow()
                .persist
                .iter()
                .find(|(_, entity)| **entity == id)
                .map(|(key, _)| key.clone()))
        })?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "is_alive",
        lua.create_function(move |_, id: i64| Ok(s.borrow().world.is_alive(entity_from_lua(id)?)))?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "destroy",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state.world.despawn(id).map_err(world_error)?;
            state.names.retain(|_, value| *value != id);
            state.persist.retain(|_, value| *value != id);
            state.health.remove(&id);
            let _ = state.physics.remove_body(id);
            let _ = state.physics3d.remove_body(id);
            Ok(true)
        })?,
    )?;
    lua.globals().set("entity", entity)?;
    let sprite = lua.create_table()?;
    let s = Rc::clone(&state);
    sprite.set(
        "set",
        lua.create_function(move |_, (id, component): (i64, String)| {
            let id = entity_from_lua(id)?;
            let component = SpriteComponent::from_component_str(&component)
                .map_err(|error| api_error(error.to_string()))?;
            let sprite_value = component
                .sprite()
                .map_err(|error| api_error(error.to_string()))?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state.world.insert(id, sprite_value).map_err(world_error)?;
            state
                .world
                .insert(id, SpriteAsset(component.texture))
                .map_err(world_error)?;
            Ok(())
        })?,
    )?;
    // 运行期读回渲染用的贴图/尺寸：`sprite.set` 会同时写 Sprite 组件与 SpriteAsset，
    // 验收驱动与工具需要能确认"实体上现在真的是哪张图"（M21 主角形象切换、
    // NPC/敌人形象诊断），否则只能间接猜测。纹理解析顺序与渲染一致：
    // Animation 的当前 clip sheet 优先，没有动画时用 SpriteAsset。
    let s = Rc::clone(&state);
    sprite.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let value = state
                .world
                .get::<crate::core::sprite::Sprite>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no sprite"))?;
            let table = lua.create_table()?;
            table.set("size_x", value.size.x)?;
            table.set("size_y", value.size.y)?;
            table.set("pivot_x", value.pivot.x)?;
            table.set("pivot_y", value.pivot.y)?;
            table.set("tint_r", value.tint[0])?;
            table.set("tint_g", value.tint[1])?;
            table.set("tint_b", value.tint[2])?;
            table.set("tint_a", value.tint[3])?;
            table.set(
                "texture",
                state
                    .world
                    .get::<SpriteAsset>(id)
                    .map_err(world_error)?
                    .map(|asset| asset.0.clone()),
            )?;
            // 有动画时渲染走 clip 的 sheet（覆盖 Sprite 的 texture），
            // 一并以 animation_sheet 给出，避免调用方以为"贴图没换"。
            if let Some(machine) = state
                .world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            {
                table.set("animation_sheet", machine.current_clip().sheet.clone())?;
            }
            Ok(table)
        })?,
    )?;
    lua.globals().set("sprite", sprite)?;

    let transform = lua.create_table()?;
    let s = Rc::clone(&state);
    transform.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let value = state
                .world
                .get::<Transform>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform"))?;
            let t = lua.create_table()?;
            t.set("x", value.position.x)?;
            t.set("y", value.position.y)?;
            t.set("rotation", value.rotation)?;
            t.set("scale_x", value.scale.x)?;
            t.set("scale_y", value.scale.y)?;
            Ok(t)
        })?,
    )?;
    let s = Rc::clone(&state);
    transform.set(
        "set",
        lua.create_function(
            move |_,
                  (id, x, y, rotation, scale_x, scale_y): (
                i64,
                f32,
                f32,
                Option<f32>,
                Option<f32>,
                Option<f32>,
            )| {
                let id = entity_from_lua(id)?;
                let mut state = s.borrow_mut();
                require_alive(&state, id)?;
                let t = state
                    .world
                    .get_mut::<Transform>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no transform"))?;
                t.position = Vec2::new(x, y);
                if let Some(v) = rotation {
                    t.rotation = v;
                }
                if let Some(v) = scale_x {
                    t.scale.x = v;
                }
                if let Some(v) = scale_y {
                    t.scale.y = v;
                }
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    transform.set(
        "move",
        lua.create_function(move |_, (id, dx, dy): (i64, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let t = state
                .world
                .get_mut::<Transform>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform"))?;
            t.position.x += dx;
            t.position.y += dy;
            Ok(())
        })?,
    )?;
    lua.globals().set("transform", transform)?;

    let transform3d = lua.create_table()?;
    let s = Rc::clone(&state);
    transform3d.set(
        "attach",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state
                .world
                .insert(id, Transform3D::default())
                .map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    transform3d.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let value = state
                .world
                .get::<Transform3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform3d"))?;
            let t = lua.create_table()?;
            t.set("x", value.position.x)?;
            t.set("y", value.position.y)?;
            t.set("z", value.position.z)?;
            t.set("rx", value.rotation.x)?;
            t.set("ry", value.rotation.y)?;
            t.set("rz", value.rotation.z)?;
            t.set("rw", value.rotation.w)?;
            t.set("sx", value.scale.x)?;
            t.set("sy", value.scale.y)?;
            t.set("sz", value.scale.z)?;
            Ok(t)
        })?,
    )?;
    let s = Rc::clone(&state);
    transform3d.set(
        "set",
        lua.create_function(
            move |_, (id, x, y, z, rx, ry, rz, rw, sx, sy, sz): Transform3dSetArgs| {
                let id = entity_from_lua(id)?;
                let mut state = s.borrow_mut();
                require_alive(&state, id)?;
                if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                    return Err(api_error("transform3d.set position must be finite"));
                }
                let t = state
                    .world
                    .get_mut::<Transform3D>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no transform3d"))?;
                t.position = Vec3::new(x, y, z);
                if let (Some(rx), Some(ry), Some(rz), Some(rw)) = (rx, ry, rz, rw) {
                    if !rx.is_finite() || !ry.is_finite() || !rz.is_finite() || !rw.is_finite() {
                        return Err(api_error("transform3d.set rotation must be finite"));
                    }
                    t.rotation = crate::core::math3d::Quat {
                        x: rx,
                        y: ry,
                        z: rz,
                        w: rw,
                    };
                }
                if let (Some(sx), Some(sy), Some(sz)) = (sx, sy, sz) {
                    if sx <= 0.0 || sy <= 0.0 || sz <= 0.0 {
                        return Err(api_error("transform3d.set scale must be positive"));
                    }
                    t.scale = Vec3::new(sx, sy, sz);
                }
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    transform3d.set(
        "move",
        lua.create_function(move |_, (id, dx, dy, dz): (i64, f32, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let t = state
                .world
                .get_mut::<Transform3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform3d"))?;
            t.position.x += dx;
            t.position.y += dy;
            t.position.z += dz;
            Ok(())
        })?,
    )?;
    lua.globals().set("transform3d", transform3d)?;

    let mesh = lua.create_table()?;
    let s = Rc::clone(&state);
    mesh.set(
        "set",
        lua.create_function(move |_, (id, path): (i64, String)| {
            let id = entity_from_lua(id)?;
            let renderer = MeshRenderer::from_component_str(&format!("mesh={path}"))
                .map_err(|error| api_error(error.to_string()))?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state.world.insert(id, renderer).map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    mesh.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(renderer) = state.world.get::<MeshRenderer>(id).map_err(world_error)? else {
                return Ok(mlua::Value::Nil);
            };
            Ok(mlua::Value::String(lua.create_string(&renderer.mesh)?))
        })?,
    )?;
    let s = Rc::clone(&state);
    mesh.set(
        "clear",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state
                .world
                .remove::<MeshRenderer>(id)
                .map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    lua.globals().set("mesh", mesh)?;

    let camera = lua.create_table()?;
    let s = Rc::clone(&state);
    camera.set(
        "set_active",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            if state
                .world
                .get::<Camera3D>(id)
                .map_err(world_error)?
                .is_none()
            {
                return Err(api_error("entity has no camera3d"));
            }
            state.active_camera = Some(id);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    camera.set(
        "get_active",
        lua.create_function(move |_, ()| Ok(s.borrow().active_camera.map(entity_to_lua)))?,
    )?;
    let s = Rc::clone(&state);
    camera.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(camera) = state.world.get::<Camera3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no camera3d"));
            };
            let t = lua.create_table()?;
            t.set("x", camera.position.x)?;
            t.set("y", camera.position.y)?;
            t.set("z", camera.position.z)?;
            t.set("tx", camera.target.x)?;
            t.set("ty", camera.target.y)?;
            t.set("tz", camera.target.z)?;
            t.set("fov", camera.fov_y.to_degrees())?;
            t.set("near", camera.near)?;
            t.set("far", camera.far)?;
            Ok(t)
        })?,
    )?;
    let s = Rc::clone(&state);
    camera.set(
        "set",
        lua.create_function(
            move |_,
                  (id, x, y, z, tx, ty, tz, fov): (
                i64,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                Option<f32>,
            )| {
                let id = entity_from_lua(id)?;
                let mut state = s.borrow_mut();
                require_alive(&state, id)?;
                if !Vec3::new(x, y, z).is_finite() || !Vec3::new(tx, ty, tz).is_finite() {
                    return Err(api_error("camera.set position/target must be finite"));
                }
                if let Some(fov) = fov
                    && (!fov.is_finite() || fov <= 0.0 || fov >= 180.0)
                {
                    return Err(api_error("camera.set fov must be in (0, 180) degrees"));
                }
                let Some(camera) = state.world.get_mut::<Camera3D>(id).map_err(world_error)? else {
                    return Err(api_error("entity has no camera3d"));
                };
                camera.position = Vec3::new(x, y, z);
                camera.target = Vec3::new(tx, ty, tz);
                if let Some(fov) = fov {
                    camera.fov_y = fov.to_radians();
                }
                Ok(true)
            },
        )?,
    )?;
    lua.globals().set("camera", camera)?;

    let light = lua.create_table()?;
    let s = Rc::clone(&state);
    light.set(
        "set_active",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            if state.world.get::<Light>(id).map_err(world_error)?.is_none() {
                return Err(api_error("entity has no light"));
            }
            state.active_light = Some(id);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    light.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(light) = state.world.get::<Light>(id).map_err(world_error)? else {
                return Err(api_error("entity has no light"));
            };
            let t = lua.create_table()?;
            t.set("dx", light.direction.x)?;
            t.set("dy", light.direction.y)?;
            t.set("dz", light.direction.z)?;
            t.set("r", light.color[0])?;
            t.set("g", light.color[1])?;
            t.set("b", light.color[2])?;
            t.set("intensity", light.intensity)?;
            Ok(t)
        })?,
    )?;
    let s = Rc::clone(&state);
    light.set(
        "set",
        lua.create_function(
            move |_,
                  (id, dx, dy, dz, r, g, b, intensity): (
                i64,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
            )| {
                let id = entity_from_lua(id)?;
                let mut state = s.borrow_mut();
                require_alive(&state, id)?;
                if !intensity.is_finite() || intensity < 0.0 {
                    return Err(api_error("light intensity must be finite and non-negative"));
                }
                let values = [dx, dy, dz, r, g, b];
                if values.iter().any(|value| !value.is_finite()) {
                    return Err(api_error("light parameters must be finite"));
                }
                let light = state
                    .world
                    .get_mut::<Light>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no light"))?;
                light.direction = Vec3::new(dx, dy, dz);
                light.color = [r, g, b];
                light.intensity = intensity;
                Ok(())
            },
        )?,
    )?;
    lua.globals().set("light", light)?;

    let health = lua.create_table()?;
    let s = Rc::clone(&state);
    health.set(
        "get",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            Ok(state.health.get(&id).copied().unwrap_or(0.0))
        })?,
    )?;
    let s = Rc::clone(&state);
    health.set(
        "set",
        lua.create_function(move |_, (id, value): (i64, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            if !value.is_finite() || value < 0.0 {
                return Err(api_error("health must be finite and non-negative"));
            }
            state.health.insert(id, value);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    health.set(
        "damage",
        lua.create_function(move |_, (id, amount): (i64, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            if !amount.is_finite() || amount < 0.0 {
                return Err(api_error("damage must be finite and non-negative"));
            }
            let value = state.health.entry(id).or_insert(0.0);
            *value = (*value - amount).max(0.0);
            Ok(*value)
        })?,
    )?;
    lua.globals().set("health", health)?;

    let tilemap = lua.create_table()?;
    let s = Rc::clone(&state);
    tilemap.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Ok(mlua::Value::Nil);
            };
            let t = lua.create_table()?;
            t.set("tile_size", map.tile_size.x)?;
            t.set("width", map.width)?;
            t.set("height", map.height)?;
            t.set("texture", map.texture.clone())?;
            Ok(mlua::Value::Table(t))
        })?,
    )?;
    // `tilemap.set_texture(id, texture)` swaps the tileset a Tilemap samples.
    // Needed for data-driven art variants (e.g. per-season ground tilesets):
    // the renderer re-reads `Tilemap::texture` every frame and rebuilds the
    // layer when the component changes, so the swap is visible next frame.
    let s = Rc::clone(&state);
    tilemap.set(
        "set_texture",
        lua.create_function(move |_, (id, texture): (i64, String)| {
            let id = entity_from_lua(id)?;
            if texture.trim().is_empty() {
                return Err(api_error("tilemap texture must not be empty"));
            }
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let map = state
                .world
                .get_mut::<Tilemap>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no tilemap"))?;
            map.texture = texture;
            Ok(())
        })?,
    )?;
    // `tilemap.create(width, height, tile_size, texture?, name?)` spawns a new
    // entity with an empty Tilemap component, so farmland grids can be authored
    // and rebuilt at runtime (M16-04) and survive scene-less/headless runs.
    let s = Rc::clone(&state);
    tilemap.set(
        "create",
        lua.create_function(
            move |_,
                  (width, height, tile_size, texture, name): (
                usize,
                usize,
                f32,
                Option<String>,
                Option<String>,
            )| {
                let mut state = s.borrow_mut();
                let mut map = Tilemap::new(Vec2::new(tile_size, tile_size), width, height)
                    .map_err(|error| api_error(error.to_string()))?;
                map.texture = texture.unwrap_or_else(|| DEFAULT_TILESET_PATH.to_owned());
                let id = state.world.spawn();
                state
                    .world
                    .insert(id, Transform::default())
                    .map_err(world_error)?;
                state.world.insert(id, map).map_err(world_error)?;
                if let Some(name) = name
                    && !name.is_empty()
                {
                    state.names.insert(name, id);
                }
                Ok(entity_to_lua(id))
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "is_solid",
        lua.create_function(move |_, (id, x, y): (i64, usize, usize)| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Err(api_error("entity has no tilemap"));
            };
            Ok(map.is_solid(x, y).unwrap_or(false))
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "get_cell",
        lua.create_function(move |_, (id, x, y): (i64, usize, usize)| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Err(api_error("entity has no tilemap"));
            };
            Ok(map.cell(x, y))
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "set_cell",
        lua.create_function(move |_, (id, x, y, value): (i64, usize, usize, u8)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let map = state
                .world
                .get_mut::<Tilemap>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no tilemap"))?;
            map.set_cell(x, y, value)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "set_solid",
        lua.create_function(move |_, (id, x, y, solid): (i64, usize, usize, bool)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            {
                let map = state
                    .world
                    .get_mut::<Tilemap>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no tilemap"))?;
                map.set_solid(x, y, solid)
                    .map_err(|error| api_error(error.to_string()))?;
            }
            refresh_tilemap_physics(&mut state, id);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "get_meta",
        lua.create_function(move |_, (id, x, y, key): (i64, usize, usize, String)| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Err(api_error("entity has no tilemap"));
            };
            Ok(map.get_meta(x, y, &key).map(str::to_owned))
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "set_meta",
        lua.create_function(
            move |_, (id, x, y, key, value): (i64, usize, usize, String, String)| {
                let id = entity_from_lua(id)?;
                let mut state = s.borrow_mut();
                require_alive(&state, id)?;
                let map = state
                    .world
                    .get_mut::<Tilemap>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no tilemap"))?;
                map.set_meta(x, y, key, value)
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(())
            },
        )?,
    )?;
    // `tilemap.find_path(id, from_x, from_y, to_x, to_y, diagonal?)` returns a
    // table of `{x=, y=}` cells (including both endpoints) or `nil, reason`.
    let s = Rc::clone(&state);
    tilemap.set(
        "find_path",
        lua.create_function(
            move |lua,
                  (id, from_x, from_y, to_x, to_y, diagonal): (
                i64,
                usize,
                usize,
                usize,
                usize,
                Option<bool>,
            )| {
                let id = entity_from_lua(id)?;
                let state = s.borrow();
                require_alive(&state, id)?;
                let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                    return Err(api_error("entity has no tilemap"));
                };
                match map.find_path((from_x, from_y), (to_x, to_y), diagonal.unwrap_or(false)) {
                    Ok(cells) => {
                        let table = lua.create_table()?;
                        for (index, (x, y)) in cells.into_iter().enumerate() {
                            let cell = lua.create_table()?;
                            cell.set("x", x)?;
                            cell.set("y", y)?;
                            table.raw_set(index + 1, cell)?;
                        }
                        Ok((mlua::Value::Table(table), mlua::Value::Nil))
                    }
                    Err(error) => Ok((
                        mlua::Value::Nil,
                        mlua::Value::String(lua.create_string(error.to_string())?),
                    )),
                }
            },
        )?,
    )?;
    lua.globals().set("tilemap", tilemap)?;

    let grid_api = lua.create_table()?;
    grid_api.set(
        "world_to_cell",
        lua.create_function(move |lua, (x, y, tile_size): (f32, f32, f32)| {
            require_positive_tile_size(tile_size)?;
            let (cx, cy) = grid::world_to_cell(x, y, tile_size);
            table_i32_pair(lua, cx, cy)
        })?,
    )?;
    grid_api.set(
        "cell_to_world",
        lua.create_function(move |lua, (cx, cy, tile_size): (i32, i32, f32)| {
            require_positive_tile_size(tile_size)?;
            let pos = grid::cell_to_world(cx, cy, tile_size);
            table_vec(lua, pos.x, pos.y)
        })?,
    )?;
    grid_api.set(
        "facing_cell",
        lua.create_function(
            move |lua, (x, y, facing, tile_size): (f32, f32, String, f32)| {
                require_positive_tile_size(tile_size)?;
                let (cx, cy) = grid::facing_cell(x, y, Facing::parse(&facing), tile_size);
                table_i32_pair(lua, cx, cy)
            },
        )?,
    )?;
    grid_api.set(
        "in_bounds",
        lua.create_function(
            move |_, (cx, cy, width, height): (i32, i32, usize, usize)| {
                Ok(grid::in_bounds(cx, cy, width, height))
            },
        )?,
    )?;
    lua.globals().set("grid", grid_api)?;

    let tag = lua.create_table()?;
    let s = Rc::clone(&state);
    tag.set(
        "get",
        lua.create_function(move |_, (id, key): (i64, String)| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            Ok(state.tags.get(&id).and_then(|bag| bag.get(&key)).cloned())
        })?,
    )?;
    let s = Rc::clone(&state);
    tag.set(
        "set",
        lua.create_function(move |_, (id, key, value): (i64, String, String)| {
            let id = entity_from_lua(id)?;
            if key.is_empty() {
                return Err(api_error("tag key must not be empty"));
            }
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            if value.is_empty() {
                if let Some(bag) = state.tags.get_mut(&id) {
                    bag.remove(&key);
                    if bag.is_empty() {
                        state.tags.remove(&id);
                    }
                }
            } else {
                state.tags.entry(id).or_default().insert(key, value);
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("tag", tag)?;

    let interact = lua.create_table()?;
    let s = Rc::clone(&state);
    interact.set(
        "query",
        lua.create_function(move |lua, (x, y, radius): (f32, f32, f32)| {
            if !radius.is_finite() || radius < 0.0 {
                return Err(api_error("interact radius must be finite and non-negative"));
            }
            let state = s.borrow();
            let mut hits: Vec<(f32, i32, i64, String, i32)> = Vec::new();
            for id in state.world.entities_with::<Transform>() {
                let Some(tags) = state.tags.get(&id) else {
                    continue;
                };
                if !(tags.contains_key("interact") || tags.contains_key("kind")) {
                    continue;
                }
                let Some(t) = state.world.get::<Transform>(id).map_err(world_error)? else {
                    continue;
                };
                let dx = t.position.x - x;
                let dy = t.position.y - y;
                let dist_sq = dx * dx + dy * dy;
                if dist_sq > radius * radius {
                    continue;
                }
                let kind = tags
                    .get("kind")
                    .cloned()
                    .or_else(|| tags.get("interact").cloned())
                    .unwrap_or_default();
                let priority = tags
                    .get("priority")
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(0);
                hits.push((dist_sq, -priority, entity_to_lua(id), kind, priority));
            }
            hits.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.cmp(&b.1))
            });
            let result = lua.create_table()?;
            for (index, (_, _, id, kind, priority)) in hits.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("id", id)?;
                entry.set("kind", kind)?;
                entry.set("priority", priority)?;
                result.set(index + 1, entry)?;
            }
            Ok(result)
        })?,
    )?;
    let s = Rc::clone(&state);
    interact.set(
        "prompt",
        lua.create_function(move |_, text: String| {
            let mut state = s.borrow_mut();
            state.interact_prompt = Some(text.clone());
            state.ui.insert(
                "__interact_prompt".to_owned(),
                UiText {
                    text,
                    visible: true,
                    position: [16.0, 48.0],
                    scale: 1.0,
                    font_size: Some(18.0),
                },
            );
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    interact.set(
        "clear_prompt",
        lua.create_function(move |_, ()| {
            let mut state = s.borrow_mut();
            state.interact_prompt = None;
            state.ui.remove("__interact_prompt");
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    interact.set(
        "get_prompt",
        lua.create_function(move |_, ()| Ok(s.borrow().interact_prompt.clone()))?,
    )?;
    lua.globals().set("interact", interact)?;

    let animation = lua.create_table()?;
    let s = Rc::clone(&state);
    animation.set(
        "get",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(machine) = state
                .world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            else {
                return Err(api_error("entity has no animation"));
            };
            Ok(machine.current_state().to_owned())
        })?,
    )?;
    let s = Rc::clone(&state);
    animation.set(
        "set",
        lua.create_function(move |_, (id, name): (i64, String)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let machine = state
                .world
                .get_mut::<AnimationStateMachine>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation"))?;
            machine
                .set_state(&name)
                .map_err(|error| api_error(error.to_string()))
        })?,
    )?;
    let s = Rc::clone(&state);
    animation.set(
        "frame",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(machine) = state
                .world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            else {
                return Err(api_error("entity has no animation"));
            };
            Ok(machine.current_frame())
        })?,
    )?;
    let s = Rc::clone(&state);
    animation.set(
        "ended",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let machine = state
                .world
                .get_mut::<AnimationStateMachine>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation"))?;
            Ok(machine.take_ended())
        })?,
    )?;
    // 运行期挂动画：`entity.spawn` 只插入 Transform，Lua 侧此前无法给实体加
    // Animation 组件（NPC/敌人/特效因此只能显示单帧）。attach 解析同一套组件串
    // （`default=..;clip:<name>:<sheet>:<cols>:<fps>:<looped>`）并插入/覆盖组件。
    let s = Rc::clone(&state);
    animation.set(
        "attach",
        lua.create_function(move |_, (id, component): (i64, String)| {
            let machine = AnimationStateMachine::from_component_str(&component)
                .map_err(|error| api_error(error.to_string()))?;
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state.world.insert(id, machine).map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    animation.set(
        "clips",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(machine) = state
                .world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            else {
                return Err(api_error("entity has no animation"));
            };
            let names = lua.create_table()?;
            for (index, clip) in machine.clips.iter().enumerate() {
                names.set(index + 1, clip.name.clone())?;
            }
            Ok(names)
        })?,
    )?;
    lua.globals().set("animation", animation)?;

    let animation3d = lua.create_table()?;
    let s = Rc::clone(&state);
    animation3d.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(animation) = state.world.get::<Animation3D>(id).map_err(world_error)? else {
                return Ok(mlua::Value::Nil);
            };
            Ok(mlua::Value::String(
                lua.create_string(animation.current_name())?,
            ))
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "set",
        lua.create_function(move |_, (id, name): (i64, String)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let animation = state
                .world
                .get_mut::<Animation3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation3d"))?;
            animation
                .set_clip(&name)
                .map_err(|error| api_error(error.to_string()))
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "list",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(animation) = state.world.get::<Animation3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no animation3d"));
            };
            let result = lua.create_table()?;
            for (index, name) in animation.clip_names().into_iter().enumerate() {
                result.set(index + 1, name)?;
            }
            Ok(result)
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "paused",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(animation) = state.world.get::<Animation3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no animation3d"));
            };
            Ok(animation.is_paused())
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "set_paused",
        lua.create_function(move |_, (id, paused): (i64, bool)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let animation = state
                .world
                .get_mut::<Animation3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation3d"))?;
            animation.set_paused(paused);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "time",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(animation) = state.world.get::<Animation3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no animation3d"));
            };
            Ok(animation.time())
        })?,
    )?;
    lua.globals().set("animation3d", animation3d)?;

    let physics = lua.create_table()?;
    let s = Rc::clone(&state);
    physics.set(
        "overlap",
        lua.create_function(move |lua, (x, y, radius): (f32, f32, f32)| {
            if !radius.is_finite() || radius < 0.0 {
                return Err(api_error("overlap radius must be finite and non-negative"));
            }
            let state = s.borrow();
            let ids = state.world.entities_with::<Transform>();
            let result = lua.create_table()?;
            let mut n = 1;
            for id in ids {
                if let Some(t) = state.world.get::<Transform>(id).map_err(world_error)? {
                    let dx = t.position.x - x;
                    let dy = t.position.y - y;
                    if dx * dx + dy * dy <= radius * radius {
                        result.set(n, entity_to_lua(id))?;
                        n += 1;
                    }
                }
            }
            Ok(result)
        })?,
    )?;
    let s = Rc::clone(&state);
    physics.set(
        "raycast",
        lua.create_function(
            move |_, (x, y, dx, dy, max_distance): (f32, f32, f32, f32, f32)| {
                if !max_distance.is_finite() || max_distance < 0.0 {
                    return Err(api_error(
                        "raycast distance must be finite and non-negative",
                    ));
                }
                let state = s.borrow();
                let mut hit = None;
                let mut best = max_distance;
                for id in state.world.entities_with::<Transform>() {
                    if let Some(t) = state.world.get::<Transform>(id).map_err(world_error)? {
                        let px = t.position.x - x;
                        let py = t.position.y - y;
                        let length = (dx * dx + dy * dy).sqrt();
                        if length > 0.0 {
                            let along = (px * dx + py * dy) / length;
                            let perp = (px * dy - py * dx).abs() / length;
                            if along >= 0.0 && along <= best && perp <= 0.5 {
                                best = along;
                                hit = Some(entity_to_lua(id));
                            }
                        }
                    }
                }
                Ok(hit)
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    physics.set(
        "set_velocity",
        lua.create_function(move |_, (id, x, y): (i64, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state
                .physics
                .set_velocity(id, x, y)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    physics.set(
        "get_velocity",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let velocity = state
                .physics
                .velocity(id)
                .map_err(|error| api_error(error.to_string()))?;
            table_vec(lua, velocity.x, velocity.y)
        })?,
    )?;
    let s = Rc::clone(&state);
    physics.set(
        "set_position",
        lua.create_function(move |_, (id, x, y): (i64, f32, f32)| {
            let id = entity_from_lua(id)?;
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("physics.set_position must be finite"));
            }
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state
                .physics
                .set_position(id, Vec2::new(x, y))
                .map_err(|error| api_error(error.to_string()))?;
            // Keep the ECS transform aligned so rendering/Lua reads agree with
            // the body until the next physics step writes positions back.
            if let Ok(Some(transform)) = state.world.get_mut::<Transform>(id) {
                transform.position = Vec2::new(x, y);
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    physics.set(
        "events",
        lua.create_function(move |lua, ()| {
            let mut state = s.borrow_mut();
            let events = state.physics.drain_contacts();
            let result = lua.create_table()?;
            for (index, contact) in events.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("a", entity_to_lua(contact.a))?;
                entry.set("b", entity_to_lua(contact.b))?;
                entry.set("started", contact.started)?;
                result.set(index + 1, entry)?;
            }
            Ok(result)
        })?,
    )?;
    lua.globals().set("physics", physics)?;

    // 3D physics (M9-02): same shape as `physics`, but bodies are created from
    // scene `Collider3d` components and positions sync to `Transform3D`.
    let physics3d = lua.create_table()?;
    let s = Rc::clone(&state);
    physics3d.set(
        "set_velocity",
        lua.create_function(move |_, (id, x, y, z): (i64, f32, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state
                .physics3d
                .set_velocity(id, x, y, z)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    physics3d.set(
        "get_velocity",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let velocity = state
                .physics3d
                .velocity(id)
                .map_err(|error| api_error(error.to_string()))?;
            let table = lua.create_table()?;
            table.set("x", velocity.x)?;
            table.set("y", velocity.y)?;
            table.set("z", velocity.z)?;
            Ok(table)
        })?,
    )?;
    let s = Rc::clone(&state);
    physics3d.set(
        "set_position",
        lua.create_function(move |_, (id, x, y, z): (i64, f32, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state
                .physics3d
                .set_position(id, Vec3::new(x, y, z))
                .map_err(|error| api_error(error.to_string()))?;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    physics3d.set(
        "events",
        lua.create_function(move |lua, ()| {
            let mut state = s.borrow_mut();
            let events = state.physics3d.drain_contacts();
            let result = lua.create_table()?;
            for (index, contact) in events.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("a", entity_to_lua(contact.a))?;
                entry.set("b", entity_to_lua(contact.b))?;
                entry.set("started", contact.started)?;
                result.set(index + 1, entry)?;
            }
            Ok(result)
        })?,
    )?;
    let s = Rc::clone(&state);
    physics3d.set(
        "raycast",
        lua.create_function(
            move |_, (ox, oy, oz, dx, dy, dz, max_distance): (f32, f32, f32, f32, f32, f32, f32)| {
                let state = s.borrow();
                let hit = state
                    .physics3d
                    .raycast(Vec3::new(ox, oy, oz), Vec3::new(dx, dy, dz), max_distance)
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(hit.map(entity_to_lua))
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    physics3d.set(
        "overlap",
        lua.create_function(move |lua, (cx, cy, cz, radius): (f32, f32, f32, f32)| {
            let state = s.borrow();
            let hits = state.physics3d.overlap(Vec3::new(cx, cy, cz), radius);
            let result = lua.create_table()?;
            for (index, id) in hits.into_iter().enumerate() {
                result.set(index + 1, entity_to_lua(id))?;
            }
            Ok(result)
        })?,
    )?;
    lua.globals().set("physics3d", physics3d)?;

    let particles = lua.create_table()?;
    let s = Rc::clone(&state);
    particles.set(
        "burst",
        lua.create_function(move |_, (x, y, count): (f32, f32, u32)| {
            s.borrow_mut().particles.burst(
                Vec2::new(x, y),
                count as usize,
                120.0,
                0.6,
                [1.0, 0.6, 0.2, 1.0],
            );
            Ok(())
        })?,
    )?;
    lua.globals().set("particles", particles)?;

    let audio = lua.create_table()?;
    let s = Rc::clone(&state);
    audio.set(
        "play",
        lua.create_function(move |_, name: String| {
            let mut state = s.borrow_mut();
            if let Err(error) = state.audio.play(&name) {
                tracing::warn!(target: "audio", name = %name, error = %error, "[audio] play failed");
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "volume",
        lua.create_function(move |_, level: f32| {
            s.borrow_mut().audio.set_volume(level);
            Ok(())
        })?,
    )?;
    // ---- M20-02：音量分组（总线）------------------------------------------
    // 总线名：master / bgm / sfx / ui；未知名报 Lua 错误（便于定位拼写错误），
    // 实际音量 = 主音量 × 分组音量（`AudioSystem::effective_volume`）。
    audio.set(
        "buses",
        lua.create_function(move |lua, ()| {
            let buses = lua.create_table()?;
            for (index, bus) in AudioBus::ALL.into_iter().enumerate() {
                buses.set(index + 1, bus.key())?;
            }
            Ok(buses)
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "set_bus_volume",
        lua.create_function(move |_, (bus, level): (String, f32)| {
            let bus = parse_audio_bus(&bus)?;
            s.borrow_mut().audio.set_bus_volume(bus, level);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "bus_volume",
        lua.create_function(move |_, bus: String| {
            let bus = parse_audio_bus(&bus)?;
            Ok(s.borrow().audio.bus_volume(bus))
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "effective_volume",
        lua.create_function(move |_, bus: String| {
            let bus = parse_audio_bus(&bus)?;
            Ok(s.borrow().audio.effective_volume(bus))
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "play_on",
        lua.create_function(move |_, (bus, name): (String, String)| {
            let bus = parse_audio_bus(&bus)?;
            if let Err(error) = s.borrow_mut().audio.play_on(bus, &name) {
                tracing::warn!(target: "audio", name = %name, bus = bus.key(), error = %error, "[audio] play_on failed");
                return Ok((false, error.to_string()));
            }
            Ok((true, String::new()))
        })?,
    )?;
    // 音量设置文本往返（只覆盖四路音量，不含工程设置文件的其余内容）。
    let s = Rc::clone(&state);
    audio.set(
        "settings_text",
        lua.create_function(move |_, ()| Ok(s.borrow().audio.to_settings_text()))?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "apply_settings",
        lua.create_function(move |_, text: String| {
            match s.borrow_mut().audio.apply_settings_text(&text) {
                Ok(()) => Ok((true, String::new())),
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;
    // 工程设置文件 `settings.toml` 的 `[audio]` 段（M20-02）：
    // 缺失文件用默认值；写入保序保留其余段（后续任务的 [input] 不会被覆盖）。
    // 路径根由 `settings_root()` 决定（P3：发布包 → 用户数据目录）。
    let s = Rc::clone(&state);
    audio.set(
        "settings_path",
        lua.create_function(move |_, ()| {
            let root = settings_root(&s.borrow());
            Ok(root.map(|root| {
                ProjectSettings::path_in(&root)
                    .to_string_lossy()
                    .into_owned()
            }))
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "load_settings",
        lua.create_function(move |_, ()| {
            let root = settings_root(&s.borrow());
            let Some(root) = root else {
                return Ok((false, "no_project_root".to_owned()));
            };
            match ProjectSettings::load_from(&root) {
                Ok(Some(settings)) => {
                    settings.apply_to(&mut s.borrow_mut().audio);
                    Ok((true, "loaded".to_owned()))
                }
                Ok(None) => Ok((false, "missing".to_owned())),
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "save_settings",
        lua.create_function(move |_, ()| {
            let root = settings_root(&s.borrow());
            let Some(root) = root else {
                return Ok((false, "no_project_root".to_owned()));
            };
            let settings = ProjectSettings::from_audio(&s.borrow().audio);
            match settings.save_to(&root) {
                Ok(()) => Ok((true, "saved".to_owned())),
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "music",
        lua.create_function(move |_, name: String| {
            let mut state = s.borrow_mut();
            if let Err(error) = state.audio.play_music(&name) {
                tracing::warn!(target: "audio", name = %name, error = %error, "[audio] music start failed");
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "stop_music",
        lua.create_function(move |_, ()| {
            s.borrow_mut().audio.stop_music();
            Ok(())
        })?,
    )?;
    lua.globals().set("audio", audio)?;

    let scene = lua.create_table()?;
    let s = Rc::clone(&state);
    scene.set(
        "load",
        lua.create_function(move |_, name: String| {
            let state = s.borrow();
            let exists = request_scene_load(&state, &name);
            if exists {
                drop(state);
                s.borrow_mut().scene_pending_load = Some(name);
            }
            Ok(exists)
        })?,
    )?;
    let s = Rc::clone(&state);
    scene.set(
        "reload",
        lua.create_function(move |_, ()| Ok(s.borrow().scene.clone()))?,
    )?;
    lua.globals().set("scene", scene)?;
    let map = lua.create_table()?;
    let s = Rc::clone(&state);
    map.set(
        "goto",
        lua.create_function(move |_, (name, x, y): (String, Option<f32>, Option<f32>)| {
            let root = {
                let state = s.borrow();
                state.project_root.clone()
            };
            let exists = root
                .as_ref()
                .is_some_and(|root| root.join("scenes").join(format!("{name}.scene")).is_file());
            if !exists {
                return Ok(false);
            }
            let mut state = s.borrow_mut();
            state.save_data.insert("__map".to_owned(), name.clone());
            if let (Some(x), Some(y)) = (x, y) {
                if !x.is_finite() || !y.is_finite() {
                    return Err(api_error("map.goto spawn must be finite"));
                }
                state
                    .save_data
                    .insert("__map:spawn:x".to_owned(), x.to_string());
                state
                    .save_data
                    .insert("__map:spawn:y".to_owned(), y.to_string());
            }
            state.scene_pending_load = Some(name);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "go",
        lua.create_function(move |_, (name, x, y): (String, Option<f32>, Option<f32>)| {
            let root = {
                let state = s.borrow();
                state.project_root.clone()
            };
            let exists = root
                .as_ref()
                .is_some_and(|root| root.join("scenes").join(format!("{name}.scene")).is_file());
            if !exists {
                return Ok(false);
            }
            let mut state = s.borrow_mut();
            state.save_data.insert("__map".to_owned(), name.clone());
            if let (Some(x), Some(y)) = (x, y) {
                if !x.is_finite() || !y.is_finite() {
                    return Err(api_error("map.go spawn must be finite"));
                }
                state
                    .save_data
                    .insert("__map:spawn:x".to_owned(), x.to_string());
                state
                    .save_data
                    .insert("__map:spawn:y".to_owned(), y.to_string());
            }
            state.scene_pending_load = Some(name);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "enter",
        lua.create_function(move |_, (name, x, y): (String, f32, f32)| {
            let root = {
                let state = s.borrow();
                state.project_root.clone()
            };
            let exists = root
                .as_ref()
                .is_some_and(|root| root.join("scenes").join(format!("{name}.scene")).is_file());
            if !exists {
                return Ok(false);
            }
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("map.enter spawn must be finite"));
            }
            let mut state = s.borrow_mut();
            state.save_data.insert("__map".to_owned(), name.clone());
            state
                .save_data
                .insert("__map:spawn:x".to_owned(), x.to_string());
            state
                .save_data
                .insert("__map:spawn:y".to_owned(), y.to_string());
            state.scene_pending_load = Some(name);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "current",
        lua.create_function(move |lua, ()| {
            let state = s.borrow();
            let current = state
                .save_data
                .get("__map")
                .cloned()
                .or_else(|| state.scene.clone());
            current
                .map(|value| lua.create_string(&value).map(Value::String))
                .transpose()
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "spawn",
        lua.create_function(move |lua, ()| {
            let state = s.borrow();
            let Some(x) = state.save_data.get("__map:spawn:x") else {
                return Ok(Value::Nil);
            };
            let Some(y) = state.save_data.get("__map:spawn:y") else {
                return Ok(Value::Nil);
            };
            let Ok(x) = x.parse::<f32>() else {
                return Ok(Value::Nil);
            };
            let Ok(y) = y.parse::<f32>() else {
                return Ok(Value::Nil);
            };
            let result = lua.create_table()?;
            result.set("x", x)?;
            result.set("y", y)?;
            Ok(Value::Table(result))
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "set_spawn",
        lua.create_function(move |_, (x, y): (f32, f32)| {
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("map.set_spawn must be finite"));
            }
            let mut state = s.borrow_mut();
            state
                .save_data
                .insert("__map:spawn:x".to_owned(), x.to_string());
            state
                .save_data
                .insert("__map:spawn:y".to_owned(), y.to_string());
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "state",
        lua.create_function(move |lua, (name, value): (String, Option<Value>)| {
            let key = format!("__mapstate:{name}");
            if let Some(value) = value {
                let json = lua_value_to_json(value)?;
                // B-3：注册 `map.schema(name, spec)` 后，写入会先校验，
                // 让错误在发生处暴露，而不是延迟到读档时才发现。
                {
                    let state = s.borrow();
                    if let Some(schema) = state.map_state_schemas.get(&name)
                        && let Some(error) = crate::core::data_schema::validate(
                            &json,
                            schema,
                            &format!("map.state {name}"),
                        )
                    {
                        return Err(api_error(error));
                    }
                }
                let encoded =
                    serde_json::to_string(&json).map_err(|error| api_error(error.to_string()))?;
                s.borrow_mut().save_data.insert(key, encoded);
                return Ok(Value::Boolean(true));
            }
            let Some(encoded) = s.borrow().save_data.get(&key).cloned() else {
                return Ok(Value::Nil);
            };
            let json = serde_json::from_str(&encoded)
                .map_err(|error| api_error(format!("map.state {name} is corrupt: {error}")))?;
            json_to_lua_value(lua, &json)
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "state_clear",
        lua.create_function(move |_, name: Option<String>| {
            let mut state = s.borrow_mut();
            let before = state.save_data.len();
            match &name {
                Some(name) => {
                    let key = format!("__mapstate:{name}");
                    state.save_data.remove(&key);
                }
                None => state
                    .save_data
                    .retain(|key, _| !key.starts_with("__mapstate:")),
            }
            Ok(before - state.save_data.len())
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "schema",
        lua.create_function(move |_, (name, spec): (String, Option<Value>)| {
            let mut state = s.borrow_mut();
            if let Some(spec) = spec {
                let json = lua_value_to_json(spec)?;
                state.map_state_schemas.insert(name, json);
            } else {
                state.map_state_schemas.remove(&name);
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("map", map)?;
    let asset = lua.create_table()?;
    let s = Rc::clone(&state);
    asset.set(
        "load",
        lua.create_function(move |_, path: String| {
            let mut state = s.borrow_mut();
            let token = format!("asset:{}", path);
            state.assets.insert(path, token.clone());
            Ok(token)
        })?,
    )?;
    let s = Rc::clone(&state);
    asset.set(
        "release",
        lua.create_function(move |_, token: String| {
            s.borrow_mut().assets.retain(|_, v| v != &token);
            Ok(())
        })?,
    )?;
    lua.globals().set("asset", asset)?;

    // Validates a freshly loaded data table against its registered schema
    // (`data.schema`), rejecting bad content with a located error instead of
    // silently caching it.
    fn validate_data_schema(
        state: &Rc<RefCell<ScriptState>>,
        path: &str,
        json: &serde_json::Value,
        verb: &str,
    ) -> Result<(), mlua::Error> {
        let schema = state.borrow().schema_defs.get(path).cloned();
        if let Some(schema) = schema
            && let Some(error) = crate::core::data_schema::validate(json, &schema, path)
        {
            return Err(api_error(format!(
                "data.{verb} {path}: schema violation: {error}"
            )));
        }
        Ok(())
    }

    let data = lua.create_table()?;
    let s = Rc::clone(&state);
    data.set(
        "load",
        lua.create_function(move |lua, path: String| {
            let root = {
                let state = s.borrow();
                state
                    .project_root
                    .clone()
                    .ok_or_else(|| api_error("data.load requires a project root"))?
            };
            if let Some(cached) = s.borrow().data_cache.get(&path) {
                return json_to_lua_value(lua, cached);
            }
            let file_path = root.join(&path);
            let bytes = std::fs::read(&file_path).map_err(|error| {
                api_error(format!("data.load {}: {error}", file_path.display()))
            })?;
            let decoded = crate::core::pack::decode(&bytes).map_err(|error| {
                api_error(format!("data.load {}: {error}", file_path.display()))
            })?;
            let text = String::from_utf8_lossy(&decoded);
            let json: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
                api_error(format!("data.load {}: {error}", file_path.display()))
            })?;
            validate_data_schema(&s, &path, &json, "load")?;
            s.borrow_mut().data_cache.insert(path, json.clone());
            json_to_lua_value(lua, &json)
        })?,
    )?;
    let s = Rc::clone(&state);
    data.set(
        "reload",
        lua.create_function(move |lua, path: String| {
            s.borrow_mut().data_cache.remove(&path);
            let root = {
                let state = s.borrow();
                state
                    .project_root
                    .clone()
                    .ok_or_else(|| api_error("data.reload requires a project root"))?
            };
            let file_path = root.join(&path);
            let bytes = std::fs::read(&file_path).map_err(|error| {
                api_error(format!("data.reload {}: {error}", file_path.display()))
            })?;
            let decoded = crate::core::pack::decode(&bytes).map_err(|error| {
                api_error(format!("data.reload {}: {error}", file_path.display()))
            })?;
            let text = String::from_utf8_lossy(&decoded);
            let json: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
                api_error(format!("data.reload {}: {error}", file_path.display()))
            })?;
            validate_data_schema(&s, &path, &json, "reload")?;
            s.borrow_mut().data_cache.insert(path, json.clone());
            json_to_lua_value(lua, &json)
        })?,
    )?;
    let s = Rc::clone(&state);
    data.set(
        "cache_clear",
        lua.create_function(move |_, ()| {
            s.borrow_mut().data_cache.clear();
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    data.set(
        "schema",
        lua.create_function(move |_, (path, spec): (String, Option<Value>)| {
            let mut state = s.borrow_mut();
            if let Some(spec) = spec {
                let json = lua_value_to_json(spec)?;
                state.schema_defs.insert(path.clone(), json);
                // A schema change must not be masked by a stale cached table.
                state.data_cache.remove(&path);
            } else {
                state.schema_defs.remove(&path);
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("data", data)?;
    let ui = lua.create_table()?;
    let s = Rc::clone(&state);
    ui.set(
        "text",
        lua.create_function(move |_, (id, text): (String, String)| {
            s.borrow_mut().ui.insert(
                id,
                UiText {
                    text,
                    visible: true,
                    position: [10.0, 10.0],
                    scale: 1.0,
                    font_size: None,
                },
            );
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "text_at",
        lua.create_function(
            move |_,
                  (id, x, y, text, scale, font_size): (
                String,
                f32,
                f32,
                String,
                Option<f32>,
                Option<f32>,
            )| {
                s.borrow_mut().ui.insert(
                    id,
                    UiText {
                        text,
                        visible: true,
                        position: [x, y],
                        scale: scale.unwrap_or(1.0),
                        font_size,
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "bar",
        lua.create_function(
            move |_,
                  (id, x, y, width, height, ratio, r, g, b): (
                String,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
            )| {
                s.borrow_mut().ui_bars.insert(
                    id,
                    UiBar {
                        position: [x, y],
                        size: [width, height],
                        ratio: ratio.clamp(0.0, 1.0),
                        color: [r, g, b, 1.0],
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "show",
        lua.create_function(move |_, id: String| {
            s.borrow_mut()
                .ui
                .entry(id)
                .or_insert(UiText {
                    text: String::new(),
                    visible: false,
                    position: [10.0, 10.0],
                    scale: 1.0,
                    font_size: None,
                })
                .visible = true;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "hide",
        lua.create_function(move |_, id: String| {
            let mut state = s.borrow_mut();
            state
                .ui
                .entry(id.clone())
                .or_insert(UiText {
                    text: String::new(),
                    visible: false,
                    position: [10.0, 10.0],
                    scale: 1.0,
                    font_size: None,
                })
                .visible = false;
            // M22：`hide` 也作用于**同 id 的动态面板**（`ui.panel` 建的矩形）。
            // 之前只认文本，于是「先 clear 再重画」的面板类 UI 永远隐藏不掉，
            // 会留下上一次选中项的残留高亮框（标题菜单的实测缺陷）。
            // 只改已存在的面板，不凭空创建（保持 `hide` 对未知 id 的宽松语义）。
            if let Some(panel) = state.ui_panels.get_mut(&id) {
                panel.visible = false;
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("ui", &ui)?;
    let s = Rc::clone(&state);
    ui.set(
        "get",
        lua.create_function(move |lua, id: String| {
            let state = s.borrow();
            let Some(value) = state.ui.get(&id) else {
                return Ok(None);
            };
            let result = lua.create_table()?;
            result.set("text", value.text.clone())?;
            result.set("visible", value.visible)?;
            Ok(Some(result))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "text_get",
        // 读回 `ui.text` / `ui.text_at` 建的动态文本（内容 + 位置 + 字号 + 可见性）；
        // 不存在返回 nil。用途（M30）：headless 里拿到"画面上真正画出来的那串字"，
        // 再用 `ui.measure_text` 量它是否超出容器——排版检查不能只查源码字面量。
        lua.create_function(move |lua, id: String| {
            let state = s.borrow();
            let Some(text) = state.ui.get(&id) else {
                return Ok(None);
            };
            let result = lua.create_table()?;
            result.set("text", text.text.clone())?;
            result.set("x", text.position[0])?;
            result.set("y", text.position[1])?;
            result.set("scale", text.scale)?;
            // 未显式给 font_size 时，渲染口径是 `scale × EM_PX`（`ui.text_effective_scale`）。
            result.set(
                "size",
                text.font_size
                    .unwrap_or(text.scale * crate::core::text::EM_PX),
            )?;
            result.set("visible", text.visible)?;
            Ok(Some(result))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "measure_text",
        // 与渲染**同口径**的文本度量（M30）：只读字形覆盖与 advance，不光栅化字形。
        // 返回 `{width, height, lines, glyphs, missing}`；`missing` 是字体（含后备字体）
        // 没有字形的字符拼接串（无缺字为空串）。
        // 为什么需要：中文排版不能按"字数 × 系数"估宽（CJK 满宽、ASCII 半宽、
        // 全角标点又不同），也不能用 `string.len` 数字节；之前 HUD 用的是
        // "CJK 记 2、其余记 1"的启发式裁剪，遇到宽拉丁字母/长数字仍会溢出。
        lua.create_function(move |lua, (text, size): (String, Option<f32>)| {
            let size_px = size.unwrap_or(crate::core::text::EM_PX);
            if !size_px.is_finite() || size_px <= 0.0 {
                return Err(api_error("ui.measure_text size must be positive"));
            }
            let probe = {
                let mut state = s.borrow_mut();
                state
                    .font_probe()
                    .map_err(|error| api_error(error.to_string()))?
            };
            let Some(probe) = probe else {
                return Err(api_error(
                    "ui.measure_text: project font unavailable (check project.toml `font`)",
                ));
            };
            let measured = crate::core::text::measure_text(
                &probe,
                &text,
                size_px,
                crate::core::text::TextStyle::default().spacing,
            );
            let result = lua.create_table()?;
            result.set("width", measured.width)?;
            result.set("height", measured.height)?;
            result.set("lines", measured.lines)?;
            result.set("glyphs", measured.glyphs)?;
            result.set("missing", measured.missing_text())?;
            result.set("missing_count", measured.missing.len())?;
            Ok(result)
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "panel_get",
        // M22：读回 `ui.panel` 建的动态面板（几何 + 颜色 + 可见性）；不存在返回 nil。
        // 用途：UI 动画/可见性的 headless 断言（例如「选中高亮确实画出来了且在弹跳」），
        // 以及运行期调试面板。只读，不改状态。
        lua.create_function(move |lua, id: String| {
            let state = s.borrow();
            let Some(panel) = state.ui_panels.get(&id) else {
                return Ok(None);
            };
            let result = lua.create_table()?;
            result.set("x", panel.position[0])?;
            result.set("y", panel.position[1])?;
            result.set("width", panel.size[0])?;
            result.set("height", panel.size[1])?;
            result.set("r", panel.color[0])?;
            result.set("g", panel.color[1])?;
            result.set("b", panel.color[2])?;
            result.set("a", panel.color[3])?;
            result.set("visible", panel.visible)?;
            Ok(Some(result))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "panel",
        lua.create_function(
            move |_,
                  (id, x, y, width, height, r, g, b, a): (
                String,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
            )| {
                if !finite_ui_rect(x, y, width, height) {
                    return Err(api_error("ui.panel rect must be finite and positive"));
                }
                let color = [r, g, b, a];
                if !finite_color4(color) {
                    return Err(api_error("ui.panel color must be finite"));
                }
                s.borrow_mut().ui_panels.insert(
                    id,
                    UiPanel {
                        position: [x, y],
                        size: [width, height],
                        color,
                        visible: true,
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "image",
        lua.create_function(
            move |_,
                  (id, x, y, width, height, texture, r, g, b): (
                String,
                f32,
                f32,
                f32,
                f32,
                String,
                f32,
                f32,
                f32,
            )| {
                if !finite_ui_rect(x, y, width, height) {
                    return Err(api_error("ui.image rect must be finite and positive"));
                }
                s.borrow_mut().ui_images.insert(
                    id,
                    DynamicUiImage {
                        texture,
                        position: [x, y],
                        size: [width, height],
                        color: [r, g, b],
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                        nine_slice: None,
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "image_get",
        // M29：读回 `ui.image` 建的动态图片（几何 + 贴图 + 颜色）；不存在返回 nil。
        // 与 `panel_get` 同一用途：让"确实画出来了"成为可 headless 断言的事实
        // （第一个真实消费者是雨天的天气图标）。
        lua.create_function(move |lua, id: String| {
            let state = s.borrow();
            let Some(image) = state.ui_images.get(&id) else {
                return Ok(None);
            };
            let result = lua.create_table()?;
            result.set("x", image.position[0])?;
            result.set("y", image.position[1])?;
            result.set("width", image.size[0])?;
            result.set("height", image.size[1])?;
            result.set("texture", image.texture.clone())?;
            result.set("r", image.color[0])?;
            result.set("g", image.color[1])?;
            result.set("b", image.color[2])?;
            Ok(Some(result))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "grid",
        lua.create_function(
            move |_,
                  (id, x, y, columns, rows, cell_size, gap): (
                String,
                f32,
                f32,
                i64,
                i64,
                f32,
                f32,
            )| {
                if columns <= 0 || rows <= 0 {
                    return Err(api_error("ui.grid columns/rows must be positive"));
                }
                if !finite_ui_rect(x, y, cell_size, cell_size) || !gap.is_finite() || gap < 0.0 {
                    return Err(api_error("ui.grid rect/gap must be finite and valid"));
                }
                let columns = columns as usize;
                let rows = rows as usize;
                let cell_count = columns * rows;
                s.borrow_mut().ui_grids.insert(
                    id,
                    UiGrid {
                        position: [x, y],
                        columns,
                        rows,
                        cell_size: [cell_size, cell_size],
                        gap,
                        background: [0.10, 0.10, 0.13, 0.92],
                        cells: vec![
                            UiGridCell {
                                texture: None,
                                color: [0.22, 0.23, 0.27, 0.95],
                            };
                            cell_count
                        ],
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "grid_cell",
        lua.create_function(
            move |_,
                  (id, index, texture, r, g, b, a): (
                String,
                i64,
                Option<String>,
                f32,
                f32,
                f32,
                f32,
            )| {
                let mut state = s.borrow_mut();
                let Some(grid) = state.ui_grids.get_mut(&id) else {
                    return Err(api_error(format!("ui.grid {id} not found")));
                };
                if index <= 0 || (index as usize) > grid.cells.len() {
                    return Err(api_error(format!(
                        "ui.grid_cell index {index} out of bounds for {id}"
                    )));
                }
                let color = [r, g, b, a];
                if !finite_color4(color) {
                    return Err(api_error("ui.grid_cell color must be finite"));
                }
                grid.cells[(index - 1) as usize] = UiGridCell { texture, color };
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "grid_clear",
        lua.create_function(move |_, id: String| {
            let mut state = s.borrow_mut();
            let Some(grid) = state.ui_grids.get_mut(&id) else {
                return Err(api_error(format!("ui.grid {id} not found")));
            };
            grid.cells.fill(UiGridCell {
                texture: None,
                color: [0.22, 0.23, 0.27, 0.95],
            });
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "grid_hit",
        lua.create_function(move |_, (id, x, y): (String, f32, f32)| {
            let state = s.borrow();
            let Some(grid) = state.ui_grids.get(&id) else {
                return Err(api_error(format!("ui.grid {id} not found")));
            };
            for index in 1..=grid.columns * grid.rows {
                let Some((origin, size)) =
                    grid.cell_rect(index, state.ui_viewport, state.ui_reference)
                else {
                    continue;
                };
                if x >= origin[0]
                    && x <= origin[0] + size[0]
                    && y >= origin[1]
                    && y <= origin[1] + size[1]
                {
                    return Ok(Some(index as i64));
                }
            }
            Ok(None)
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "tooltip",
        lua.create_function(
            move |_, (id, x, y, text, width, height): (String, f32, f32, String, f32, f32)| {
                if !finite_ui_rect(x, y, width, height) {
                    return Err(api_error("ui.tooltip rect must be finite and positive"));
                }
                let mut state = s.borrow_mut();
                state.ui_panels.insert(
                    id.clone(),
                    UiPanel {
                        position: [x, y],
                        size: [width, height],
                        color: [0.08, 0.09, 0.12, 0.94],
                        visible: true,
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                    },
                );
                state.ui.insert(
                    format!("{id}:tooltip"),
                    UiText {
                        text,
                        visible: true,
                        position: [x + 8.0, y + 4.0],
                        scale: 1.0,
                        font_size: Some(14.0),
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "tooltip_hide",
        lua.create_function(move |_, id: String| {
            let mut state = s.borrow_mut();
            if let Some(panel) = state.ui_panels.get_mut(&id) {
                panel.visible = false;
            }
            if let Some(text) = state.ui.get_mut(&format!("{id}:tooltip")) {
                text.visible = false;
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "modal",
        lua.create_function(
            move |_, (id, x, y, width, height, title): (String, f32, f32, f32, f32, String)| {
                if !finite_ui_rect(x, y, width, height) {
                    return Err(api_error("ui.modal rect must be finite and positive"));
                }
                let mut state = s.borrow_mut();
                state.ui_panels.insert(
                    id.clone(),
                    UiPanel {
                        position: [x, y],
                        size: [width, height],
                        color: [0.12, 0.13, 0.17, 0.96],
                        visible: true,
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                    },
                );
                state.ui.insert(
                    format!("{id}:title"),
                    UiText {
                        text: title,
                        visible: true,
                        position: [x + 12.0, y + 8.0],
                        scale: 1.0,
                        font_size: Some(20.0),
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "modal_show",
        lua.create_function(move |_, id: String| {
            let mut state = s.borrow_mut();
            if let Some(panel) = state.ui_panels.get_mut(&id) {
                panel.visible = true;
            }
            if let Some(text) = state.ui.get_mut(&format!("{id}:title")) {
                text.visible = true;
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "modal_hide",
        lua.create_function(move |_, id: String| {
            let mut state = s.borrow_mut();
            if let Some(panel) = state.ui_panels.get_mut(&id) {
                panel.visible = false;
            }
            if let Some(text) = state.ui.get_mut(&format!("{id}:title")) {
                text.visible = false;
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "clear",
        lua.create_function(move |_, id: String| {
            let mut state = s.borrow_mut();
            state.ui_panels.remove(&id);
            state.ui_images.remove(&id);
            state.ui_grids.remove(&id);
            state.ui.remove(&format!("{id}:tooltip"));
            state.ui.remove(&format!("{id}:title"));
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "drag_begin",
        lua.create_function(move |_, (id, index): (String, i64)| {
            if index <= 0 {
                return Err(api_error("ui.drag_begin index must be positive"));
            }
            let mut state = s.borrow_mut();
            if !state.ui_grids.contains_key(&id) {
                return Err(api_error(format!("ui.grid {id} not found")));
            }
            state.ui_drag = Some(UiDrag {
                from_id: id,
                from_index: index as usize,
            });
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "drag_end",
        lua.create_function(move |lua, (id, index): (String, i64)| {
            if index <= 0 {
                return Err(api_error("ui.drag_end index must be positive"));
            }
            let drag = s.borrow().ui_drag.clone();
            let Some(drag) = drag else {
                return Ok(false);
            };
            s.borrow_mut().ui_drag = None;
            let callback: Value = lua.globals().get("on_drag")?;
            if let Value::Function(function) = callback {
                function.call::<()>((drag.from_id, drag.from_index as i64, id, index))?;
            }
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "drag_active",
        lua.create_function(move |lua, ()| {
            let state = s.borrow();
            let Some(drag) = &state.ui_drag else {
                return Ok(Value::Nil);
            };
            let result = lua.create_table()?;
            result.set("id", drag.from_id.clone())?;
            result.set("index", drag.from_index as i64)?;
            Ok(Value::Table(result))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "drag_cancel",
        lua.create_function(move |_, ()| {
            s.borrow_mut().ui_drag = None;
            Ok(())
        })?,
    )?;

    // ---- M20-02：UI 焦点与滚动导航（core/ui_navigation.rs 的运行时接线）----
    // 面板用方向键/摇杆方向驱动列表选择与滚动；序号统一 0 基（Lua 面板常用 1 基，
    // 由游戏侧 `panel_nav.lua` 转换）。`focus_open` 按 `columns` 布局项矩形，
    // 使网格面板（背包 2 列）的左右移动与单列列表的上下移动都可用。
    let s = Rc::clone(&state);
    ui.set(
        "focus_open",
        lua.create_function(move |_, (id, count, columns): (String, i64, Option<i64>)| {
            let count = count.clamp(0, 4096) as usize;
            let columns = columns.unwrap_or(1).clamp(1, 64) as usize;
            let items: Vec<FocusItem> = (0..count)
                .map(|index| {
                    let column = index % columns;
                    let line = index / columns;
                    FocusItem {
                        id: index as u64 + 1,
                        rect: [column as f32, line as f32, 1.0, 1.0],
                    }
                })
                .collect();
            let mut state = s.borrow_mut();
            match state.focus_rings.get_mut(&id) {
                Some(ring) => ring.set_items(items),
                None => {
                    state.focus_rings.insert(id, FocusRing::new(items));
                }
            }
            Ok(count as i64)
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "focus_close",
        lua.create_function(move |_, id: String| {
            Ok(s.borrow_mut().focus_rings.remove(&id).is_some())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "focus_count",
        lua.create_function(move |_, id: String| {
            Ok(s.borrow()
                .focus_rings
                .get(&id)
                .map_or(0_i64, |ring| ring.len() as i64))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "focus_select",
        lua.create_function(move |_, (id, index): (String, i64)| {
            if index < 0 {
                return Ok(false);
            }
            let mut state = s.borrow_mut();
            let Some(ring) = state.focus_rings.get_mut(&id) else {
                return Ok(false);
            };
            Ok(ring.set_current(index as u64 + 1))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "focus_index",
        lua.create_function(move |_, id: String| {
            let state = s.borrow();
            let Some(ring) = state.focus_rings.get(&id) else {
                return Ok(None);
            };
            Ok(ring
                .current()
                .and_then(|item| usize::try_from(item.id).ok())
                .and_then(|id| id.checked_sub(1))
                .map(|index| index as i64))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "focus_move",
        lua.create_function(move |_, (id, direction): (String, String)| {
            let direction = match direction.trim().to_ascii_lowercase().as_str() {
                "up" => NavDirection::Up,
                "down" => NavDirection::Down,
                "left" => NavDirection::Left,
                "right" => NavDirection::Right,
                other => {
                    return Err(api_error(format!(
                        "ui.focus_move direction must be up/down/left/right, got {other}"
                    )));
                }
            };
            let mut state = s.borrow_mut();
            let Some(ring) = state.focus_rings.get_mut(&id) else {
                return Ok(None);
            };
            Ok(ring
                .move_focus(direction)
                .and_then(|item| usize::try_from(item.id).ok())
                .and_then(|id| id.checked_sub(1))
                .map(|index| index as i64))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "focus_cycle",
        lua.create_function(move |_, (id, forward): (String, Option<bool>)| {
            let mut state = s.borrow_mut();
            let Some(ring) = state.focus_rings.get_mut(&id) else {
                return Ok(None);
            };
            Ok(ring
                .cycle(forward.unwrap_or(true))
                .and_then(|item| usize::try_from(item.id).ok())
                .and_then(|id| id.checked_sub(1))
                .map(|index| index as i64))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_open",
        lua.create_function(move |_, (id, viewport, content): (String, f32, f32)| {
            s.borrow_mut()
                .scroll_states
                .insert(id, ScrollState::new(viewport, content));
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_close",
        lua.create_function(move |_, id: String| {
            Ok(s.borrow_mut().scroll_states.remove(&id).is_some())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_extents",
        lua.create_function(move |_, (id, viewport, content): (String, f32, f32)| {
            let mut state = s.borrow_mut();
            let scroll = state.scroll_states.entry(id).or_default();
            scroll.set_extents(viewport, content);
            Ok(scroll.offset())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_offset",
        lua.create_function(move |_, id: String| {
            Ok(s.borrow().scroll_states.get(&id).map(ScrollState::offset))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_max",
        lua.create_function(move |_, id: String| {
            Ok(s.borrow()
                .scroll_states
                .get(&id)
                .map(ScrollState::max_offset))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_by",
        lua.create_function(move |_, (id, delta): (String, f32)| {
            let mut state = s.borrow_mut();
            let scroll = state.scroll_states.entry(id).or_default();
            Ok(scroll.scroll_by(delta))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_lines",
        lua.create_function(move |_, (id, lines, line_height): (String, f32, f32)| {
            let mut state = s.borrow_mut();
            let scroll = state.scroll_states.entry(id).or_default();
            Ok(scroll.scroll_lines(lines, line_height))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_page",
        lua.create_function(move |_, (id, pages): (String, f32)| {
            let mut state = s.borrow_mut();
            let scroll = state.scroll_states.entry(id).or_default();
            Ok(scroll.page(pages))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_to_visible",
        lua.create_function(move |_, (id, item_top, item_height): (String, f32, f32)| {
            let mut state = s.borrow_mut();
            let scroll = state.scroll_states.entry(id).or_default();
            Ok(scroll.scroll_to_visible(item_top, item_height))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_visible",
        lua.create_function(move |lua, id: String| {
            let state = s.borrow();
            let Some(scroll) = state.scroll_states.get(&id) else {
                return Ok(Value::Nil);
            };
            let (start, stop) = scroll.visible_range();
            let result = lua.create_table()?;
            // 不能叫 `end`：Lua 里 `range.end` 是关键字，取值必须写 `range['end']`。
            result.set("start", start)?;
            result.set("stop", stop)?;
            Ok(Value::Table(result))
        })?,
    )?;

    let item = lua.create_table()?;
    let s = Rc::clone(&state);
    item.set(
        "define",
        lua.create_function(move |_, (id, definition): (String, Table)| {
            if id.is_empty() {
                return Err(api_error("item.define id must not be empty"));
            }
            let name = definition
                .get::<Option<String>>("name")?
                .unwrap_or_else(|| id.clone());
            let description = definition
                .get::<Option<String>>("description")?
                .unwrap_or_default();
            let kind = definition
                .get::<Option<String>>("kind")?
                .unwrap_or_default();
            let texture = definition
                .get::<Option<String>>("texture")?
                .unwrap_or_default();
            let stack = definition.get::<Option<i64>>("stack")?.unwrap_or(99).max(1) as u32;
            let buy_price = definition.get::<Option<i64>>("buy")?.unwrap_or(0);
            let sell_price = definition.get::<Option<i64>>("sell")?.unwrap_or(0);
            s.borrow_mut().item_defs.insert(
                id.clone(),
                ItemDefinition {
                    id,
                    name,
                    description,
                    stack,
                    buy_price,
                    sell_price,
                    texture,
                    kind,
                },
            );
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    item.set(
        "def",
        lua.create_function(move |lua, id: String| {
            let state = s.borrow();
            let Some(definition) = state.item_defs.get(&id) else {
                return Ok(Value::Nil);
            };
            let result = lua.create_table()?;
            result.set("id", definition.id.clone())?;
            result.set("name", definition.name.clone())?;
            result.set("description", definition.description.clone())?;
            result.set("stack", definition.stack as i64)?;
            result.set("buy", definition.buy_price)?;
            result.set("sell", definition.sell_price)?;
            result.set("texture", definition.texture.clone())?;
            result.set("kind", definition.kind.clone())?;
            Ok(Value::Table(result))
        })?,
    )?;
    lua.globals().set("item", item)?;

    let inv = lua.create_table()?;
    let s = Rc::clone(&state);
    inv.set(
        "create",
        lua.create_function(move |_, (id, capacity): (String, Option<i64>)| {
            if id.is_empty() {
                return Err(api_error("inv.create id must not be empty"));
            }
            let capacity = capacity.unwrap_or(24).max(1) as usize;
            s.borrow_mut()
                .inventories
                .insert(id.clone(), Inventory::new(capacity));
            Ok(id)
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "hotbar",
        lua.create_function(move |_, (id, size): (String, Option<i64>)| {
            if id.is_empty() {
                return Err(api_error("inv.hotbar id must not be empty"));
            }
            let capacity = size.unwrap_or(9).max(1) as usize;
            s.borrow_mut()
                .inventories
                .insert(id.clone(), Inventory::new(capacity));
            Ok(id)
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "capacity",
        lua.create_function(move |_, id: String| {
            let state = s.borrow();
            state
                .inventories
                .get(&id)
                .map(|inventory| inventory.capacity as i64)
                .ok_or_else(|| api_error(format!("inventory {id} not found")))
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "exists",
        lua.create_function(move |_, id: String| Ok(s.borrow().inventories.contains_key(&id)))?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "add",
        lua.create_function(move |_, (id, item_id, count): (String, String, i64)| {
            if item_id.is_empty() {
                return Err(api_error("inv.add item id must not be empty"));
            }
            if count <= 0 {
                return Err(api_error("inv.add count must be positive"));
            }
            let mut state = s.borrow_mut();
            if !state.inventories.contains_key(&id) {
                state.inventories.insert(id.clone(), Inventory::new(24));
            }
            let max_stack = state
                .item_defs
                .get(&item_id)
                .map_or(99, |definition| definition.stack);
            let inventory = state
                .inventories
                .get_mut(&id)
                .expect("inventory just inserted");
            Ok(inventory.add(&item_id, count as u32, max_stack) as i64)
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "remove",
        lua.create_function(move |_, (id, item_id, count): (String, String, i64)| {
            if count <= 0 {
                return Err(api_error("inv.remove count must be positive"));
            }
            let mut state = s.borrow_mut();
            let Some(inventory) = state.inventories.get_mut(&id) else {
                return Ok(0);
            };
            Ok(inventory.remove(&item_id, count as u32) as i64)
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "count",
        lua.create_function(move |_, (id, item_id): (String, String)| {
            let state = s.borrow();
            Ok(state
                .inventories
                .get(&id)
                .map_or(0, |inventory| inventory.count(&item_id)) as i64)
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "get",
        lua.create_function(move |lua, id: String| {
            let state = s.borrow();
            let Some(inventory) = state.inventories.get(&id) else {
                return Err(api_error(format!("inventory {id} not found")));
            };
            let result = lua.create_table()?;
            result.set("id", id.clone())?;
            result.set("capacity", inventory.capacity as i64)?;
            let slots = lua.create_table()?;
            let mut occupied = 0;
            for (index, stack) in inventory.slots.iter().enumerate() {
                if stack.item_id.is_empty() {
                    continue;
                }
                let slot = lua.create_table()?;
                slot.set("item", stack.item_id.clone())?;
                slot.set("count", stack.count as i64)?;
                slots.set(index + 1, slot)?;
                occupied += 1;
            }
            result.set("slots", slots)?;
            result.set("occupied", occupied)?;
            Ok(result)
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "slot",
        lua.create_function(move |lua, (id, index): (String, i64)| {
            if index <= 0 {
                return Err(api_error("inv.slot index must be positive"));
            }
            let state = s.borrow();
            let Some(inventory) = state.inventories.get(&id) else {
                return Err(api_error(format!("inventory {id} not found")));
            };
            let Some(stack) = inventory.get((index - 1) as usize) else {
                return Err(api_error(format!(
                    "inventory {id} slot {index} out of bounds"
                )));
            };
            if stack.item_id.is_empty() {
                return Ok(Value::Nil);
            }
            let slot = lua.create_table()?;
            slot.set("item", stack.item_id.clone())?;
            slot.set("count", stack.count as i64)?;
            Ok(Value::Table(slot))
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "set",
        lua.create_function(
            move |_, (id, index, item_id, count): (String, i64, Option<String>, Option<i64>)| {
                if index <= 0 {
                    return Err(api_error("inv.set index must be positive"));
                }
                let stack = match (item_id, count) {
                    (Some(item_id), Some(count)) if count > 0 => {
                        ItemStack::new(item_id, count as u32)
                    }
                    _ => ItemStack::default(),
                };
                let mut state = s.borrow_mut();
                let Some(inventory) = state.inventories.get_mut(&id) else {
                    return Err(api_error(format!("inventory {id} not found")));
                };
                inventory
                    .set((index - 1) as usize, stack)
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "swap",
        lua.create_function(
            move |_, (from_id, from_index, to_id, to_index): (String, i64, String, i64)| {
                swap_inventories(&mut s.borrow_mut(), &from_id, from_index, &to_id, to_index)
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "move",
        lua.create_function(
            move |_,
                  (from_id, from_index, to_id, to_index, count): (
                String,
                i64,
                String,
                Option<i64>,
                Option<i64>,
            )| {
                move_inventories(
                    &mut s.borrow_mut(),
                    &from_id,
                    from_index,
                    &to_id,
                    to_index,
                    count,
                )
            },
        )?,
    )?;
    // Equipment skeleton (M16-07): named slots holding an item id. No runtime
    // effect yet — stat/gear systems consume this in M18-00.
    let s = Rc::clone(&state);
    inv.set(
        "equip",
        lua.create_function(
            move |_, (id, slot, item_id): (String, String, Option<String>)| {
                if id.is_empty() || slot.is_empty() {
                    return Err(api_error(
                        "inv.equip needs an inventory id and a non-empty slot",
                    ));
                }
                let mut state = s.borrow_mut();
                let Some(inventory) = state.inventories.get_mut(&id) else {
                    return Err(api_error(format!("inventory {id} not found")));
                };
                inventory.equip(&slot, item_id.as_deref().unwrap_or(""));
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "equip_get",
        lua.create_function(move |_, (id, slot): (String, String)| {
            let state = s.borrow();
            let Some(inventory) = state.inventories.get(&id) else {
                return Err(api_error(format!("inventory {id} not found")));
            };
            Ok(inventory.equip_get(&slot).map(str::to_owned))
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "equip_list",
        lua.create_function(move |lua, id: String| {
            let state = s.borrow();
            let Some(inventory) = state.inventories.get(&id) else {
                return Err(api_error(format!("inventory {id} not found")));
            };
            let result = lua.create_table()?;
            for (slot, item) in &inventory.equipment {
                result.set(slot.clone(), item.clone())?;
            }
            Ok(result)
        })?,
    )?;
    lua.globals().set("inv", inv)?;

    let container = lua.create_table()?;
    let s = Rc::clone(&state);
    container.set(
        "open",
        lua.create_function(move |_, (entity_id, capacity): (i64, Option<i64>)| {
            let key = format!("container:{entity_id}");
            let capacity = capacity.unwrap_or(12).max(1) as usize;
            s.borrow_mut()
                .inventories
                .insert(key.clone(), Inventory::new(capacity));
            Ok(key)
        })?,
    )?;
    let s = Rc::clone(&state);
    container.set(
        "get",
        lua.create_function(move |_, entity_id: i64| {
            let key = format!("container:{entity_id}");
            let exists = s.borrow().inventories.contains_key(&key);
            if exists { Ok(Some(key)) } else { Ok(None) }
        })?,
    )?;
    let s = Rc::clone(&state);
    container.set(
        "close",
        lua.create_function(move |_, entity_id: i64| {
            let key = format!("container:{entity_id}");
            Ok(s.borrow_mut().inventories.remove(&key).is_some())
        })?,
    )?;
    let s = Rc::clone(&state);
    container.set(
        "move",
        lua.create_function(
            move |_, (entity_id, from_index, to_id, to_index): (i64, i64, String, i64)| {
                let from_id = format!("container:{entity_id}");
                move_inventories(
                    &mut s.borrow_mut(),
                    &from_id,
                    from_index,
                    &to_id,
                    Some(to_index),
                    None,
                )
            },
        )?,
    )?;
    lua.globals().set("container", container)?;

    // `shop` catalogs and price queries (M16-10). Catalogs are data-driven from
    // data/shop.json; buy/sell transactions and the money balance stay in Lua
    // (game logic), with `shop.price` as the engine's price/catalog service.
    let shop = lua.create_table()?;
    let s = Rc::clone(&state);
    shop.set(
        "set_catalog",
        lua.create_function(move |_, (name, spec): (String, Value)| {
            let json = lua_value_to_json(spec)?;
            s.borrow_mut().shops.insert(name, json);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    shop.set(
        "price",
        lua.create_function(move |lua, (name, item_id): (String, String)| {
            let state = s.borrow();
            let Some(catalog) = state.shops.get(&name) else {
                return Ok(Value::Nil);
            };
            let Some(item) = catalog.get(item_id.as_str()) else {
                return Ok(Value::Nil);
            };
            let buy = item
                .get("buy")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let sell = item
                .get("sell")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let result = lua.create_table()?;
            result.set("buy", buy)?;
            result.set("sell", sell)?;
            Ok(Value::Table(result))
        })?,
    )?;
    lua.globals().set("shop", shop)?;

    let save = lua.create_table()?;
    let s = Rc::clone(&state);
    save.set(
        "write",
        lua.create_function(move |_, (key, value): (String, String)| {
            // 空键会让整份存档在下次 `load` 时无法解析（`decode` 报 empty key），
            // 因此必须在写入点就拒绝，而不是等到玩家读档时整档报废。
            if key.is_empty() {
                return Err(api_error("save.write key must not be empty"));
            }
            s.borrow_mut().save_data.insert(key, value);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "read",
        lua.create_function(move |_, key: String| Ok(s.borrow().save_data.get(&key).cloned()))?,
    )?;
    // 「开始新游戏」必须能丢弃**引擎侧**的持久键（`__map` / `__map:spawn:*` /
    // `__mapstate:*` / `__blob:*`）。这些键不属于任何游戏侧 provider，若残留，
    // 新档会读回上一局的地图、农田、建筑与制作状态（同进程内「继续 → 玩 →
    // 返回标题 → 新游戏」即可复现）。只清内存；磁盘文件在下一次 `commit` 时
    // 被整体替换。
    let s = Rc::clone(&state);
    save.set(
        "clear",
        lua.create_function(move |_, ()| {
            let mut state = s.borrow_mut();
            let removed = state.save_data.len();
            state.save_data.clear();
            tracing::info!(target: "save", "[save] cleared {removed} in-memory key(s)");
            Ok(removed)
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "blob_write",
        lua.create_function(move |_, (key, value): (String, Value)| {
            if key.is_empty() {
                return Err(api_error("save.blob_write key must not be empty"));
            }
            let json = lua_value_to_json(value)?;
            let encoded =
                serde_json::to_string(&json).map_err(|error| api_error(error.to_string()))?;
            s.borrow_mut()
                .save_data
                .insert(format!("__blob:{key}"), encoded);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "blob_read",
        lua.create_function(move |lua, key: String| {
            let state = s.borrow();
            let Some(encoded) = state.save_data.get(&format!("__blob:{key}")) else {
                return Ok(Value::Nil);
            };
            let json = serde_json::from_str(encoded)
                .map_err(|error| api_error(format!("save.blob_read {key} is corrupt: {error}")))?;
            json_to_lua_value(lua, &json)
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "slot",
        lua.create_function(move |_, slot: Option<String>| {
            s.borrow_mut().save_slot = slot.filter(|value| !value.is_empty());
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "slot_get",
        lua.create_function(move |lua, ()| {
            let slot = s.borrow().save_slot.clone();
            slot.map(|value| lua.create_string(&value).map(Value::String))
                .transpose()
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "commit",
        lua.create_function(move |_, ()| {
            let state = s.borrow();
            let Some(path) = state.save_path.clone() else {
                tracing::warn!(target: "save", "[save] no save path configured; commit ignored");
                return Ok(false);
            };
            let path = save_path_for_slot(&path, state.save_slot.as_deref());
            let text = encode_save(&state.save_data);
            let result = atomic_write(&path, &text);
            match &result {
                Ok(()) => tracing::info!(target: "save", path = %path.display(), "[save] committed"),
                Err(error) => {
                    tracing::error!(target: "save", path = %path.display(), error = %error, "[save] commit failed")
                }
            }
            Ok(result.is_ok())
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "load",
        lua.create_function(move |lua, ()| {
            let (path, slot) = {
                let state = s.borrow();
                (state.save_path.clone(), state.save_slot.clone())
            };
            let Some(path) = path else {
                tracing::warn!(target: "save", "[save] no save path configured; load ignored");
                return Ok((
                    Value::Boolean(false),
                    Value::String(lua.create_string("no_save_path")?),
                ));
            };
            let path = save_path_for_slot(&path, slot.as_deref());
            let Ok(text) = std::fs::read_to_string(&path) else {
                // 还没有存档不是错误；但必须与「存档已损坏」区分开，否则玩家与
                // 诊断面板无法判断进度是真的没了还是从没存过。
                return Ok((
                    Value::Boolean(false),
                    Value::String(lua.create_string("missing")?),
                ));
            };
            match crate::core::save::decode(&text) {
                Ok(data) => {
                    s.borrow_mut().save_data = data;
                    tracing::info!(target: "save", path = %path.display(), "[save] loaded");
                    Ok((Value::Boolean(true), Value::Nil))
                }
                Err(error) => {
                    // 损坏/版本不符：保留原文件（绝不覆盖），并把原因交给调用方。
                    tracing::warn!(target: "save", path = %path.display(), error = %error, "[save] corrupt save ignored");
                    let reason = format!("corrupt: {error}");
                    Ok((
                        Value::Boolean(false),
                        Value::String(lua.create_string(&reason)?),
                    ))
                }
            }
        })?,
    )?;
    lua.globals().set("save", save)?;

    let app = lua.create_table()?;
    let s = state;
    app.set(
        "quit",
        lua.create_function(move |_, ()| {
            s.borrow_mut().quit_requested = true;
            Ok(())
        })?,
    )?;
    lua.globals().set("app", app)?;

    // Generic named event bus: `event.emit(name, data?)` dispatches to every
    // callback registered with `event.on`/`event.subscribe` under `name`.
    // Subscriptions live in a per-VM map (not in `ScriptState`) because Lua
    // callbacks are owned by the VM that created them: hot reload builds a fresh
    // VM and re-runs `on_create`, which re-subscribes on the new VM.
    let event = lua.create_table()?;
    let subscribers: Rc<RefCell<BTreeMap<String, Vec<Function>>>> =
        Rc::new(RefCell::new(BTreeMap::new()));

    let subs = Rc::clone(&subscribers);
    event.set(
        "subscribe",
        lua.create_function(move |_, (name, callback): (String, Function)| {
            subs.borrow_mut().entry(name).or_default().push(callback);
            Ok(())
        })?,
    )?;
    let subs = Rc::clone(&subscribers);
    event.set(
        "on",
        lua.create_function(move |_, (name, callback): (String, Function)| {
            subs.borrow_mut().entry(name).or_default().push(callback);
            Ok(())
        })?,
    )?;

    let subs = Rc::clone(&subscribers);
    event.set(
        "emit",
        lua.create_function(move |_, (name, data): (String, Option<Value>)| {
            let data = data.unwrap_or(Value::Nil);
            // Snapshot the listener list first so a callback may itself emit,
            // subscribe, or unsubscribe without tripping the shared borrow.
            let callbacks = subs.borrow().get(&name).cloned().unwrap_or_default();
            for callback in callbacks {
                callback.call::<Value>((data.clone(),))?;
            }
            Ok(())
        })?,
    )?;

    let subs = Rc::clone(&subscribers);
    event.set(
        "unsubscribe",
        lua.create_function(move |_, (name, callback): (String, Option<Function>)| {
            let mut map = subs.borrow_mut();
            let Some(callback) = callback else {
                map.remove(&name);
                return Ok(());
            };
            if let Some(listeners) = map.get_mut(&name) {
                let target = callback.to_pointer();
                listeners.retain(|listener| listener.to_pointer() != target);
            }
            Ok(())
        })?,
    )?;
    let subs = Rc::clone(&subscribers);
    event.set(
        "off",
        lua.create_function(move |_, (name, callback): (String, Option<Function>)| {
            let mut map = subs.borrow_mut();
            let Some(callback) = callback else {
                map.remove(&name);
                return Ok(());
            };
            if let Some(listeners) = map.get_mut(&name) {
                let target = callback.to_pointer();
                listeners.retain(|listener| listener.to_pointer() != target);
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("event", event)?;
    Ok(())
}

fn world_error(error: WorldError) -> mlua::Error {
    api_error(error.to_string())
}

/// 输入层诊断（M20-07）：只保留最近 [`INPUT_DIAGNOSTIC_LIMIT`] 条，最新在后。
fn push_input_diagnostic(state: &mut ScriptState, message: String) {
    const INPUT_DIAGNOSTIC_LIMIT: usize = 16;
    tracing::warn!(target: "input", "{}", message);
    if state.input_diagnostics.len() >= INPUT_DIAGNOSTIC_LIMIT {
        state.input_diagnostics.remove(0);
    }
    state.input_diagnostics.push(message);
}

/// 玩家设置根目录：显式设置的 `settings_root`（发布包 = 用户数据目录）优先，
/// 否则退回项目根（开发工程形态）。所有 `settings.toml` 读写都必须走这里，
/// 否则发布包会出现"读用户目录、写程序目录"这种两边不一致的坏行为。
fn settings_root(state: &ScriptState) -> Option<PathBuf> {
    state
        .settings_root
        .clone()
        .or_else(|| state.project_root.clone())
}

/// 解析 `input.load_settings/save_settings` 的可选路径参数（M20-07）。
///
/// * 省略 → `<settings_root>/settings.toml`（无根返回 `None`，Lua 侧报
///   `no_project_root`）；
/// * 相对路径 → 相对设置根解析（无根返回 `None`）；
/// * 绝对路径 → 原样使用（无头/测试场景可给绝对路径）。
fn resolve_input_settings_path(state: &ScriptState, path: Option<&str>) -> Option<PathBuf> {
    match path.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => {
            let candidate = PathBuf::from(value);
            if candidate.is_absolute() {
                Some(candidate)
            } else {
                settings_root(state).map(|root| root.join(candidate))
            }
        }
        None => settings_root(state).map(|root| ProjectSettings::path_in(&root)),
    }
}

/// Lua `input.is_down/just_pressed/just_released` 的键名解析（M22/W1 起**严格**）。
///
/// 接受三种写法，与设置文件 / 编辑器捕获共用同一套键身份：
///
/// 1. **v2 物理键名**：`input.is_down("keye")`、`input.is_down("digit1")`、
///    `input.is_down("arrowup")`、`input.is_down("space")`（即 [`KeyCode::to_text`]
///    的输出，大小写不敏感，`key:` 前缀可省）；
/// 2. **v1 兼容名**：`"w"`、`"1"`、`"up"`、`"f1"`、`"space"`、`"escape"`、
///    `"other:1048577"`——老脚本与既有 headless 用例继续可用，按 US-QWERTY 位置
///    换算（见 [`KeyCode::v1_key_code`]）；
/// 3. `"key:<名字>"` 前缀形式（与 `input.rebind` 的绑定文本同源）。
///
/// **未知名报 Lua 错误**：旧实现把任何不认识的键静默映射成 `Other(0)`，于是拼错的
/// 键名与真实按键无法区分（`input.is_down("W ")` 会永远返回 false 且毫无提示）。
/// 现在与 `Binding::parse` 同一严格度，错误信息里带上原值。
fn parse_key_strict(name: &str) -> LuaResult<KeyCode> {
    let raw = name.trim();
    if raw.is_empty() {
        return Err(api_error("input key name must not be empty"));
    }
    let value = raw.strip_prefix("key:").unwrap_or(raw);
    if let Some(key) = KeyCode::from_text(value) {
        return Ok(key);
    }
    let mut notes = Vec::new();
    if let Some(key) = KeyCode::v1_key_code(value, &mut notes) {
        return Ok(key);
    }
    Err(api_error(format!(
        "unknown input key: {raw}（可用物理键名如 keye/digit1/arrowup/space，或旧名如 w/1/up）"
    )))
}

/// 音量总线名 → [`AudioBus`]；未知名报 Lua 错误（拼写错误可定位，不静默）。
fn parse_audio_bus(name: &str) -> LuaResult<AudioBus> {
    AudioBus::from_key(&name.trim().to_ascii_lowercase())
        .ok_or_else(|| api_error(format!("unknown audio bus: {name}")))
}
fn parse_mouse_button(button: &str) -> MouseButton {
    match button.to_ascii_lowercase().as_str() {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        _ => MouseButton::Other(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::sprite::Sprite;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 一帧「左摇杆上推」输入（含数字态，模拟 `GamepadPoller::poll_into` 的产物）。
    fn held_stick_up(value: f32) -> InputSnapshot {
        let mut input = InputSnapshot::default();
        input.set_gamepad_axis(1, value);
        input.set_gamepad_axis_digital(1, crate::core::input::AxisDirection::Positive, true);
        input
    }
    fn script(contents: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        // 进程内单调计数 + 纳秒时间戳：并行测试在同一时间刻度内调用时
        // 也不会拿到同一路径（曾因纯时间戳碰撞互相覆盖脚本导致偶发失败）。
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let unique = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("p2-lua-{id}-{unique}.lua"));
        fs::write(&path, contents).unwrap();
        path
    }

    fn production_sample_root() -> PathBuf {
        crate::core::games_dir().join("examples").join("game_api")
    }

    #[test]
    fn registers_log_time_input_and_quit_api() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_delta(0.25);
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyD, true);
        runtime.set_input(input);
        let path = script(
            "assert(time.delta() == 0.25)\nassert(input.is_down('d'))\nassert(input.axis('horizontal') == 1.0)\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        runtime.lua.load("app.quit()").exec().unwrap();
        assert!(runtime.quit_requested());
        let _ = fs::remove_file(path);
    }

    /// M22/W3：`input.action_repeat` 是**时间制**——同一段按住时长内的步数与帧率
    /// 无关（旧的帧计数规则在 60 步里只会走 6 步，这里应约 9 步）。输入帧由真实
    /// [`crate::host::gamepad::GamepadPoller`] 生产：轴按下边沿来自设备层。
    #[test]
    fn action_repeat_is_time_based_for_analog_holds() {
        use crate::host::gamepad::{FakeBackend, GamepadFrame, GamepadPoller};

        fn run_repeat(steps: usize, delay_ms: u32, repeat_ms: u32) -> Vec<usize> {
            let mut runtime = LuaRuntime::new().unwrap();
            runtime.set_nav_settings(NavSettings::new(0.5, delay_ms, repeat_ms));
            runtime.set_delta(1.0 / 60.0);

            // 第 1 帧空闲（建立基线），之后持续推住左摇杆上推。
            let mut idle = GamepadFrame::idle();
            idle.axes[crate::host::gamepad::axis::LEFT_Y] = 0.0;
            let mut held = GamepadFrame::idle();
            held.axes[crate::host::gamepad::axis::LEFT_Y] = 0.9;
            let mut poller = GamepadPoller::new(Box::new(FakeBackend::new(vec![idle, held])));

            let mut fired = Vec::new();
            for step in 1..=steps {
                let mut input = InputSnapshot::default();
                poller.poll_into(&mut input);
                runtime.set_input(input);
                if runtime.action_repeat("ui_up") {
                    fired.push(step);
                }
            }
            fired
        }

        let fired = run_repeat(61, 100, 50);
        assert_eq!(
            fired.first(),
            Some(&2),
            "轴按下边沿那一帧应立即走一步：{fired:?}"
        );
        assert!(
            !fired.iter().any(|step| (3..=7).contains(step)),
            "100ms 延迟内不得连发：{fired:?}"
        );
        // 1000ms：首次 + (1000-100)/50 ≈ 19 步；旧的帧计数规则只有 6 步。
        assert!(
            (15..=22).contains(&fired.len()),
            "时间制连发步数异常：{} 步 {fired:?}",
            fired.len()
        );

        // 间隔改为 500ms 后步数显著下降（证明确实读的是毫秒设置）。
        let slow = run_repeat(61, 100, 500);
        assert!(
            (1..=4).contains(&slow.len()),
            "500ms 间隔在 1s 内只应有约 3 步，实际 {} 步 {slow:?}",
            slow.len()
        );
    }

    /// M22/W3：「已按住但没有按下边沿」不补步——同一根摇杆同时驱动 `move_*` 与
    /// `ui_*` 时（玩家把移动改绑到摇杆），消费了边沿的那一帧走一步、下一帧的
    /// `ui_*` 路径**不得**再补一步（只会按连发延迟继续）。
    #[test]
    fn action_repeat_does_not_double_step_without_edge() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_nav_settings(NavSettings::new(0.5, 50, 50));
        runtime.set_delta(1.0 / 60.0);

        // 边沿已在别处被消费：连续三帧「按住但无边沿」都不得走步。
        for frame in 1..=3 {
            runtime.set_input(held_stick_up(0.9));
            assert!(
                !runtime.action_repeat("ui_up"),
                "第 {frame} 帧无边沿不得补步"
            );
        }
        // 第 4 帧（约 50ms 后）到连发延迟 → 正常连发一步。
        runtime.set_input(held_stick_up(0.9));
        assert!(runtime.action_repeat("ui_up"), "到延迟后应正常连发");

        // 边沿帧才是「立即一步」（设备层在越过阈值的那一帧调 `gamepad_axis_event`，
        // 它同时写数字态与边沿）。
        let mut edge = InputSnapshot::default();
        edge.set_gamepad_axis(1, 0.9);
        edge.gamepad_axis_event(1, crate::core::input::AxisDirection::Positive, true);
        let mut fresh = LuaRuntime::new().unwrap();
        fresh.set_nav_settings(NavSettings::new(0.5, 50, 50));
        fresh.set_delta(1.0 / 60.0);
        fresh.set_input(edge);
        assert!(fresh.action_repeat("ui_up"), "有边沿立即走一步");
    }

    /// M22/W3：数字量绑定（键盘/DPad）按住**不连发**——保持 M20-08 的
    /// 「一次按下 = 一步」菜单语义。
    #[test]
    fn action_repeat_does_not_repeat_digital_holds() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_nav_settings(NavSettings::new(0.5, 50, 50));
        runtime.set_delta(1.0 / 60.0);
        let mut fires = 0;
        for _ in 1..=120 {
            let mut input = InputSnapshot::default();
            input.key_event(KeyCode::ArrowUp, true);
            runtime.set_input(input);
            if runtime.action_repeat("ui_up") {
                fires += 1;
            }
        }
        assert_eq!(fires, 1, "键盘按住只应走一步");
    }

    /// M22/W4：**一步之内完成的「按下 + 松开」也必须走一步**。
    ///
    /// 边沿就是「刚按下」，与「此刻是否仍按住」无关。编辑器 Play 预览把同一帧内的
    /// 按下/松开发成 down + up 边沿（`game_view::bridge_preview_input`），若这里要求
    /// `down` 才认边沿，那一下就会被整帧丢掉——用户实测的「编辑器里按上下键切换按钮
    /// 有时没反应、发布 game.exe 正常」正是这条路径（game.exe 用事件流，按下与松开
    /// 通常落在不同逻辑步）。
    #[test]
    fn action_repeat_honors_a_tap_released_within_one_step() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_nav_settings(NavSettings::new(0.5, 50, 50));
        runtime.set_delta(1.0 / 60.0);

        // 一步内：按下 + 松开 → 快照里 `held` 为空、`pressed` 有该键。
        let mut tap = InputSnapshot::default();
        tap.key_event(KeyCode::ArrowUp, true);
        tap.key_event(KeyCode::ArrowUp, false);
        assert!(!tap.is_down(KeyCode::ArrowUp), "帧末应已是松开态");
        assert!(tap.just_pressed(KeyCode::ArrowUp));
        runtime.set_input(tap);
        assert!(
            runtime.action_repeat("ui_up"),
            "有按下边沿就必须走一步（哪怕同一步内已经松开）"
        );

        // 松开帧（无边沿、未按住）不得再走一步，也不得残留状态。
        runtime.set_input(InputSnapshot::default());
        assert!(
            !runtime.action_repeat("ui_up"),
            "无边沿的一步不得补步（一次按下 = 一步）"
        );
    }
    #[test]
    fn lua_gameplay_api_moves_damages_and_destroys() {
        let path = script(
            "player=entity.spawn('player')\ntransform.set(player,0,0)\ntransform.move(player,2,3)\nlocal p=transform.get(player)\nassert(p.x==2 and p.y==3)\nhealth.set(player,10)\nassert(health.damage(player,4)==6)\nassert(entity.find('player')==player)\nentity.destroy(player)\nassert(not entity.is_alive(player))\nfunction update() return 'ok' end",
        );
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_file(path);
    }
    #[test]
    fn entity_persist_id_registers_looks_up_and_rejects_duplicates() {
        let path = script(
            "local a = entity.spawn(nil, 'house')\n\
             assert(entity.find_id('house') == a)\n\
             assert(entity.persist_id(a) == 'house')\n\
             assert(not pcall(function() entity.spawn(nil, 'house') end))\n\
             entity.destroy(a)\n\
             assert(entity.find_id('house') == nil)\n\
             function update() return 'ok' end",
        );
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_file(path);
    }
    #[test]
    fn tilemap_find_path_returns_path_or_diagnostic() {
        let path = script(
            "function on_create()\n\
             local m = tilemap.create(4, 3, 32)\n\
             tilemap.set_solid(m, 1, 0, true)\n\
             tilemap.set_solid(m, 2, 0, true)\n\
             local cells, err = tilemap.find_path(m, 0, 0, 3, 0, false)\n\
             assert(err == nil)\n\
             assert(#cells == 6)\n\
             assert(cells[1].x == 0 and cells[1].y == 0)\n\
             assert(cells[#cells].x == 3 and cells[#cells].y == 0)\n\
             local blocked, reason = tilemap.find_path(m, 0, 0, 1, 0, false)\n\
             assert(blocked == nil and reason ~= nil)\n\
             return 'ok' end\n\
             function update() return 'ok' end",
        );
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_file(path);
    }
    #[test]
    fn event_bus_subscribes_emits_and_unsubscribes() {
        let path = script(
            "function on_create()\n\
             local received = {}\n\
             local cb = function(d) table.insert(received, d) end\n\
             local cb2 = function(d) table.insert(received, d * 10) end\n\
             event.on('ItemAdded', cb)\n\
             event.subscribe('ItemAdded', cb2)\n\
             event.emit('ItemAdded', 5)\n\
             assert(#received == 2)\n\
             assert(received[1] == 5 and received[2] == 50)\n\
             event.off('ItemAdded', cb)\n\
             received = {}\n\
             event.emit('ItemAdded', 7)\n\
             assert(#received == 1 and received[1] == 70)\n\
             event.unsubscribe('ItemAdded')\n\
             received = {}\n\
             event.emit('ItemAdded', 9)\n\
             assert(#received == 0)\n\
             event.emit('AttackHit')\n\
             local nested = 0\n\
             event.on('Outer', function() nested = nested + 1; event.emit('Inner') end)\n\
             event.on('Inner', function() nested = nested + 1 end)\n\
             event.emit('Outer')\n\
             assert(nested == 2)\n\
             return 'ok' end\n\
             function update() return 'ok' end",
        );
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_file(path);
    }
    #[test]
    fn instantiated_scene_entities_are_shared_with_lua_movement() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("test");
        scene.add_entity(SceneEntityId(1), "Player", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Sprite".into(),
                "texture=assets/textures/red-1x1.png;size=32x32".into(),
            );

        let mut runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

        let path = script(
            "function update()\nlocal p = entity.find('Player')\nassert(p ~= nil)\ntransform.move(p, 10, 5)\nreturn 'moved' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "moved");

        let player = runtime.with_world(|world| world.entities_with::<Transform>());
        assert_eq!(player.len(), 1);
        let player = player[0];
        assert!(runtime.with_world(|world| world.has::<Sprite>(player).unwrap()));
        assert!(runtime.with_world(|world| world.has::<SpriteAsset>(player).unwrap()));
        let asset =
            runtime.with_world(|world| world.get::<SpriteAsset>(player).unwrap().unwrap().clone());
        assert_eq!(asset.path(), "assets/textures/red-1x1.png");
        let moved = runtime.with_world(|world| *world.get::<Transform>(player).unwrap().unwrap());
        assert_eq!(moved.position, Vec2::new(10.0, 5.0));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn wasd_input_axis_moves_the_instantiated_player() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("test");
        scene.add_entity(SceneEntityId(1), "Player", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
            );

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.instantiate_scene(&scene).unwrap();
        let path = script(
            "local speed = 10\nfunction update()\nlocal p = entity.find('Player')\nlocal h = input.axis('horizontal')\nlocal v = input.axis('vertical')\ntransform.move(p, h * speed, v * speed)\nreturn 'moved' end",
        );
        runtime.execute_project_script(&path).unwrap();

        let player = runtime.with_world(|world| world.entities_with::<Transform>());
        assert_eq!(player.len(), 1);
        let player = player[0];

        // Hold D -> horizontal axis = +1 -> player moves right.
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyD, true);
        runtime.set_input(input);
        runtime.call_update().unwrap();
        let position =
            runtime.with_world(|world| world.get::<Transform>(player).unwrap().unwrap().position);
        assert_eq!(position.x, 10.0);
        assert_eq!(position.y, 0.0);

        // Release D, hold W -> vertical axis = +1 -> player moves up only.
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyW, true);
        runtime.set_input(input);
        runtime.call_update().unwrap();
        let position =
            runtime.with_world(|world| world.get::<Transform>(player).unwrap().unwrap().position);
        assert_eq!(position.x, 10.0);
        assert_eq!(position.y, 10.0);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn instantiated_tilemap_is_queryable_from_lua() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("test");
        scene.add_entity(SceneEntityId(1), "Map", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Tilemap".into(),
                "tile_size=32x32;width=4;height=3;tiles=100101000100;solid=100000000000".into(),
            );

        let mut runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

        let path = script(
            "function on_create()\nlocal m = entity.find('Map')\nlocal d = tilemap.get(m)\nassert(d ~= nil)\nassert(d.width == 4 and d.height == 3 and d.tile_size == 32)\nassert(tilemap.is_solid(m, 0, 0))\nassert(not tilemap.is_solid(m, 1, 1))\nassert(not tilemap.is_solid(m, 9, 9))\nassert(tilemap.get_cell(m, 3, 0) == 1)\nassert(tilemap.get_cell(m, 1, 0) == 0)\nreturn 'ok' end\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");

        let maps = runtime.with_world(|world| world.entities_with::<Tilemap>());
        assert_eq!(maps.len(), 1);
        let map =
            runtime.with_world(|world| world.get::<Tilemap>(maps[0]).unwrap().unwrap().clone());
        assert_eq!(map.width, 4);
        assert_eq!(map.is_solid(0, 0), Some(true));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn instantiated_animation_is_switchable_from_lua() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("test");
        scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
        scene.entities.get_mut(&SceneEntityId(1)).unwrap().components.insert(
            "Animation".into(),
            "default=idle;clip:idle:assets/idle-sheet.png:4:4:true;clip:attack:assets/attack-sheet.png:2:2:false".into(),
        );

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.instantiate_scene(&scene).unwrap();

        let first = script(
            "function on_create()\nlocal h = entity.find('Hero')\nassert(animation.get(h) == 'idle')\nassert(animation.set(h, 'attack'))\nassert(animation.get(h) == 'attack')\nassert(not animation.set(h, 'attack'))\nreturn 'ok' end\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&first).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");

        // advance past the 1.0s non-looping attack clip
        let machines = runtime.with_world(|world| world.entities_with::<AnimationStateMachine>());
        assert_eq!(machines.len(), 1);
        runtime.with_world_mut(|world| {
            world
                .get_mut::<AnimationStateMachine>(machines[0])
                .unwrap()
                .unwrap()
                .advance(1.1);
        });

        let second = script(
            "function update()\nlocal h = entity.find('Hero')\nassert(animation.frame(h) == 1)\nassert(animation.ended(h))\nassert(not animation.ended(h))\nreturn 'done' end",
        );
        runtime.execute_project_script(&second).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "done");
        let _ = fs::remove_file(first);
        let _ = fs::remove_file(second);
    }

    #[test]
    fn runtime_spawned_entity_can_attach_animation() {
        // 运行期创建的实体（entity.spawn 只插入 Transform）此前无法播动画：
        // NPC/敌人/特效只能显示单帧。animation.attach 让 Lua 侧自己挂组件。
        let first = script(
            "function on_create()\n\
             local id = entity.spawn('Effect', 'fx_probe')\n\
             assert(animation.attach(id, 'default=idle_s;clip:idle_s:assets/fx.png:4:8:true'))\n\
             assert(animation.get(id) == 'idle_s')\n\
             local clips = animation.clips(id)\n\
             assert(#clips == 1 and clips[1] == 'idle_s')\n\
             assert(animation.set(id, 'idle_s') == false)\n\
             return 'ok' end\n\
             function update() return 'ok' end",
        );
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&first).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");

        // 组件确实进了 ECS，且每帧推进（4 帧 @8fps → 0.2s 后到第 2 帧）
        let machines = runtime.with_world(|world| world.entities_with::<AnimationStateMachine>());
        assert_eq!(machines.len(), 1);
        runtime.with_world_mut(|world| {
            world
                .get_mut::<AnimationStateMachine>(machines[0])
                .unwrap()
                .unwrap()
                .advance(0.26);
        });
        let second = script(
            "function update()\nlocal id = entity.find_id('fx_probe')\nassert(animation.frame(id) == 2)\nreturn 'frame2' end",
        );
        runtime.execute_project_script(&second).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "frame2");

        // 非法组件串要被拒绝，且不破坏已有组件
        let third = script(
            "function update()\n\
             local id = entity.find_id('fx_probe')\n\
             local ok = pcall(animation.attach, id, 'clip:broken')\n\
             assert(not ok, 'malformed component must be rejected')\n\
             assert(animation.get(id) == 'idle_s')\n\
             local gone = pcall(animation.attach, 999999, 'default=a;clip:a:assets/fx.png:2:8:true')\n\
             assert(not gone, 'dead entity must be rejected')\n\
             return 'guarded' end",
        );
        runtime.execute_project_script(&third).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "guarded");
        let _ = fs::remove_file(first);
        let _ = fs::remove_file(second);
        let _ = fs::remove_file(third);
    }

    #[test]
    fn physics_blocks_player_on_tilemap_wall_and_emits_events() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("test");
        scene.add_entity(SceneEntityId(1), "Player", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Collider".into(), "shape=ball;radius=10".into());
        scene.add_entity(SceneEntityId(2), "Map", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
            );
        // 2x2 tilemap, right column solid -> a wall spanning x=20..40
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Tilemap".into(),
                "tile_size=20x20;width=2;height=2;tiles=0000;solid=0101".into(),
            );

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.instantiate_scene(&scene).unwrap();

        let path = script(
            "function update()\nlocal p = entity.find('Player')\nphysics.set_velocity(p, 40, 0)\nreturn 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        runtime.call_update().unwrap();
        for _ in 0..60 {
            runtime.step_physics(1.0 / 60.0);
        }

        let player = runtime.with_world(|world| world.entities_with::<Transform>());
        assert_eq!(player.len(), 2);
        let player = player[0];
        let position =
            runtime.with_world(|world| world.get::<Transform>(player).unwrap().unwrap().position);
        assert!(
            position.x < 15.0,
            "player was not blocked by the tilemap wall: x={}",
            position.x
        );

        let events = script(
            "function update()\nlocal events = physics.events()\nreturn tostring(#events) end",
        );
        runtime.execute_project_script(&events).unwrap();
        let count = runtime.call_update().unwrap();
        let count: u32 = count.parse().unwrap();
        assert!(count > 0, "no collision events reached Lua");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(events);
    }

    #[test]
    fn physics3d_body_falls_onto_static_ground_and_syncs_transform3d() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("3d");
        scene.add_entity(SceneEntityId(1), "Ball", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform3D".into(),
                "position=0,5,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Collider3d".into(), "shape=ball;radius=1".into());
        scene.add_entity(SceneEntityId(2), "Ground", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Transform3D".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        // Static ground: cuboid spanning y=-2..0 (top at y=0), centered at (0,-1,0).
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Collider3d".into(),
                "shape=cuboid;half_width=10;half_height=1;half_z=10;offset=0,-1,0;dynamic=false"
                    .into(),
            );

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.instantiate_scene(&scene).unwrap();

        let path = script(
            "function update()\nlocal b = entity.find('Ball')\nphysics3d.set_velocity(b, 0, 0, 2)\nreturn 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        runtime.call_update().unwrap();
        for _ in 0..180 {
            runtime.step_physics3d(1.0 / 60.0);
        }

        let positions: Vec<_> = runtime.with_world(|world| {
            world
                .entities_with::<Transform3D>()
                .into_iter()
                .filter_map(|id| {
                    world
                        .get::<Transform3D>(id)
                        .ok()
                        .flatten()
                        .map(|t| t.position)
                })
                .collect()
        });
        assert_eq!(positions.len(), 2);
        let ball_pos = positions.iter().find(|p| p.y > 0.5).copied().unwrap();
        // Ball radius 1 resting on the ground top (y=0) => y ≈ 1; it also moved +z.
        assert!(
            (ball_pos.y - 1.0).abs() < 0.1,
            "ball should rest at y≈1, got y={}",
            ball_pos.y
        );
        assert!(
            ball_pos.z > 2.0,
            "ball should have moved in +z, got z={}",
            ball_pos.z
        );

        // Collision events reached Lua via the physics3d table.
        let events = script(
            "function update()\nlocal events = physics3d.events()\nreturn tostring(#events) end",
        );
        runtime.execute_project_script(&events).unwrap();
        let count = runtime.call_update().unwrap();
        let count: u32 = count.parse().unwrap();
        assert!(count > 0, "no 3D collision events reached Lua");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(events);
    }

    #[test]
    fn ui_particles_and_audio_lua_api() {
        let mut runtime = LuaRuntime::new().unwrap();
        let path = script(
            "particles.burst(5, -3, 10)\nui.bar('hp', 20, 30, 100, 10, 0.5, 1, 0, 0)\nui.text_at('objective', 20, 20, 'Reach the door')\naudio.volume(0.5)\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        runtime.call_update().unwrap();
        runtime.step_particles(0.1);

        let count = runtime.with_particles(|particles| particles.len());
        assert!(count >= 10, "particles.burst should spawn at least 10");
        let (texts, bars) = runtime.with_ui(|texts, bars| (texts.clone(), bars.clone()));
        let text = texts.get("objective").expect("objective text stored");
        assert_eq!(text.text, "Reach the door");
        let bar = bars.get("hp").expect("hp bar stored");
        assert_eq!(bar.ratio, 0.5);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn ui_text_at_accepts_optional_scale_and_font_size() {
        let mut runtime = LuaRuntime::new().unwrap();
        let path = script(
            "ui.text_at('small', 10, 10, 'a')\nui.text_at('big', 10, 30, 'b', 3)\nui.text_at('huge', 10, 50, 'c', nil, 24)\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        runtime.call_update().unwrap();
        let texts = runtime.with_ui(|texts, _| texts.clone());
        assert_eq!(
            texts.get("small").expect("small text stored").scale,
            1.0,
            "no scale argument defaults to 1.0"
        );
        assert_eq!(
            texts.get("small").expect("small text stored").font_size,
            None,
            "no font_size argument stays None"
        );
        assert_eq!(
            texts.get("big").expect("big text stored").scale,
            3.0,
            "optional scale argument is honored"
        );
        let huge = texts.get("huge").expect("huge text stored");
        assert_eq!(huge.scale, 1.0, "scale stays default when font_size given");
        assert_eq!(
            huge.font_size,
            Some(24.0),
            "optional font_size argument is honored"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn project_script_can_require_sibling_lua_modules() {
        let root = std::env::temp_dir().join(format!("p2-module-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("helper.lua"),
            "local M = {}\nfunction M.answer() return 42 end\nreturn M\n",
        )
        .unwrap();
        let main = root.join("main.lua");
        fs::write(
            &main,
            "local h = require('helper')\nassert(h.answer() == 42)\nfunction update() return 'ok' end\n",
        )
        .unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&main).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn encrypted_project_and_modules_load_via_decode_and_require() {
        let root = std::env::temp_dir().join(format!("p2-enc-module-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        // 用发布包格式（压缩+加密）写模块与主脚本。
        let helper_src = "local M = {}\nfunction M.answer() return 7 end\nreturn M\n".as_bytes();
        fs::write(
            root.join("helper.lua"),
            crate::core::pack::encode(helper_src, true, true),
        )
        .unwrap();
        let main = root.join("main.lua");
        let main_src = "local h = require('helper')\nassert(h.answer() == 7)\nfunction update() return 'enc-ok' end\n";
        fs::write(
            &main,
            crate::core::pack::encode(main_src.as_bytes(), true, true),
        )
        .unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&main).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "enc-ok");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn grid_tilemap_tag_and_interact_lua_api() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("farm");
        scene.add_entity(SceneEntityId(1), "FarmMap", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Tilemap".into(),
                "tile_size=32x32;width=4;height=3;texture=assets/tileset.png;tiles=000000000000;solid=000000000000"
                    .into(),
            );
        scene.add_entity(SceneEntityId(2), "Farmer", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=16;y=16;rotation=0;scale_x=1;scale_y=1".into(),
            );
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.instantiate_scene(&scene).unwrap();
        let path = script(
            "local map = entity.find('FarmMap')\n\
             local player = entity.find('Farmer')\n\
             local cell = grid.world_to_cell(48, 16, 32)\n\
             assert(cell[1] == 1 and cell[2] == 0)\n\
             local front = grid.facing_cell(16, 16, 'right', 32)\n\
             assert(front[1] == 1 and front[2] == 0)\n\
             assert(grid.cell_to_world(1, 0, 32)[1] == 48)\n\
             tilemap.set_cell(map, 1, 0, 2)\n\
             tilemap.set_solid(map, 1, 0, true)\n\
             tilemap.set_meta(map, 1, 0, 'tilled', 'true')\n\
             assert(tilemap.get_meta(map, 1, 0, 'tilled') == 'true')\n\
             assert(tilemap.is_solid(map, 1, 0))\n\
             assert(tilemap.get(map).texture == 'assets/tileset.png')\n\
             tilemap.set_texture(map, 'assets/art/tilesets/manor/manor_ground_winter.png')\n\
             assert(tilemap.get(map).texture == 'assets/art/tilesets/manor/manor_ground_winter.png')\n\
             assert(not pcall(function() tilemap.set_texture(map, '  ') end))\n\
             assert(not pcall(function() tilemap.set_texture(player, 'assets/tileset.png') end))\n\
             tag.set(player, 'kind', 'farmer')\n\
             local hits = interact.query(16, 16, 100)\n\
             assert(#hits >= 1 and hits[1].kind == 'farmer')\n\
             interact.prompt('按 E 使用工具')\n\
             assert(interact.get_prompt() == '按 E 使用工具')\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn clock_save_slots_blobs_map_inventory_and_data_lua_api() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = std::env::temp_dir().join(format!("p2-farm-api-{}", std::process::id()));
        let scenes = root.join("scenes");
        let data = root.join("data");
        fs::create_dir_all(&scenes).unwrap();
        fs::create_dir_all(&data).unwrap();
        fs::write(
            data.join("crops.json"),
            r#"{"tomato":{"name":"Tomato","sell":5,"buy":10,"stack":8}}"#,
        )
        .unwrap();
        let mut town = Scene::new("town");
        town.add_entity(SceneEntityId(1), "Spawn", None).unwrap();
        town.save(scenes.join("town.scene")).unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        runtime.set_save_path(root.join(".data").join("save.sav"));
        runtime.set_ui_viewport(1280, 720);
        let path = script(
            "local now = clock.now()\n\
             assert(now.day == 1 and now.minute == 360)\n\
             clock.set_rate(60)\n\
             clock.pause(false)\n\
             save.slot('slot1')\n\
             save.write('hp', '42')\n\
             assert(save.commit())\n\
             assert(save.slot_get() == 'slot1')\n\
             save.blob_write('farm', { tilled = { true }, day = 2 })\n\
             local farm = save.blob_read('farm')\n\
             assert(farm.tilled[1] == true and farm.day == 2)\n\
             item.define('tomato', { name = 'Tomato', stack = 8, sell = 5, buy = 10 })\n\
             assert(item.def('tomato').sell == 5)\n\
             local bag = inv.create('bag', 4)\n\
             assert(inv.add('bag', 'tomato', 3) == 0)\n\
             assert(inv.count('bag', 'tomato') == 3)\n\
             assert(inv.move('bag', 1, 'bag', 2, nil) == 3)\n\
             assert(inv.slot('bag', 2).count == 3)\n\
             assert(inv.move('bag', 2, 'bag', 1, nil) == 3)\n\
             local box = container.open(88, 2)\n\
             assert(inv.move('bag', 1, box, 1, nil) == 3)\n\
             assert(inv.count('bag', 'tomato') == 0)\n\
             assert(inv.count(box, 'tomato') == 3)\n\
             assert(inv.swap(box, 1, 'bag', 1))\n\
             assert(inv.count(box, 'tomato') == 0)\n\
             inv.equip('bag', 'tool', 'hoe')\n\
             assert(inv.equip_get('bag', 'tool') == 'hoe')\n\
             assert(inv.equip_list('bag').tool == 'hoe')\n\
             inv.equip('bag', 'tool', nil)\n\
             assert(inv.equip_get('bag', 'tool') == nil)\n\
             assert(map.go('town', 120, 80))\n\
             assert(map.current() == 'town')\n\
             local spawn = map.spawn()\n\
             assert(spawn.x == 120 and spawn.y == 80)\n\
             map.state('farm', { tilled = true })\n\
             assert(map.state('farm').tilled == true)\n\
             local crops = data.load('data/crops.json')\n\
             assert(crops.tomato.name == 'Tomato' and crops.tomato.sell == 5)\n\
             assert(data.reload('data/crops.json').tomato.buy == 10)\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        runtime.step_clock(1.0);
        runtime
            .execute_console("assert(clock.now().minute == 420)")
            .unwrap();
        assert!(root.join(".data").join("save_slot1.sav").is_file());
        assert!(!root.join(".data").join("save.sav").is_file());
        runtime.process_scene_transition().unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn map_state_schema_rejects_invalid_writes_and_can_clear() {
        let mut runtime = LuaRuntime::new().unwrap();
        let path = script(
            "map.schema('farm', { ['$kind'] = 'object', \
             fields = { version = 'integer', \
                        tiles = { ['$kind'] = 'array', items = 'integer' }, \
                        solid = { ['$kind'] = 'array', items = 'boolean' }, \
                        meta = { ['$kind'] = 'map', values = 'any' } }, \
             required = { 'version', 'tiles', 'solid' } })\n\
             assert(map.state('farm', { version = 1, tiles = { 0 }, solid = { false }, meta = {} }))\n\
             local good = map.state('farm')\n\
             assert(good.version == 1 and good.tiles[1] == 0 and good.solid[1] == false)\n\
             local ok1, err1 = pcall(function() map.state('farm', { version = 'x' }) end)\n\
             assert(not ok1 and tostring(err1):find('map.state farm'), tostring(err1))\n\
             local ok2, err2 = pcall(function() map.state('farm', { version = 1, tiles = { 'a' }, solid = { false } }) end)\n\
             assert(not ok2 and tostring(err2):find('tiles'), tostring(err2))\n\
             local ok3, err3 = pcall(function() map.state('farm', { version = 1, tiles = { 0 } }) end)\n\
             assert(not ok3 and tostring(err3):find('missing required field'), tostring(err3))\n\
             map.schema('farm', nil)\n\
             assert(map.state('farm', { anything = true }))\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn map_state_tilemap_snapshot_survives_scene_change_and_restart() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = std::env::temp_dir().join(format!("p2-mapstate-b3-{}", std::process::id()));
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).unwrap();
        let save_path = root.join("save.sav");
        let _ = fs::remove_file(&save_path);

        // farm.scene 包含 4x3 Tilemap，初始 tile/solid 全为空。
        let mut farm = Scene::new("farm");
        farm.add_entity(SceneEntityId(1), "FarmMap", None).unwrap();
        let farm_entity = farm.entities.get_mut(&SceneEntityId(1)).unwrap();
        farm_entity.components.insert(
            "Transform".into(),
            "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
        );
        farm_entity.components.insert(
            "Tilemap".into(),
            "tile_size=32x32;width=4;height=3;tiles=000000000000;solid=000000000000".into(),
        );
        farm.save(scenes.join("farm.scene")).unwrap();
        let mut home = Scene::new("home");
        home.add_entity(SceneEntityId(1), "Home", None).unwrap();
        home.save(scenes.join("home.scene")).unwrap();

        // 启动脚本注册分地图 schema 与快照/恢复 helper，模拟游戏侧
        // `map_state.lua` 的职责：脚本维护 tile/meta/solid 格式，引擎用
        // `map.schema` 校验，并存入 `__mapstate:<name>`。
        let boot = "map.schema('farm', { ['$kind'] = 'object', \
                    fields = { version = 'integer', \
                               tiles = { ['$kind'] = 'array', items = 'integer' }, \
                               solid = { ['$kind'] = 'array', items = 'boolean' }, \
                               meta = { ['$kind'] = 'map', values = 'any' } }, \
                    required = { 'version', 'tiles', 'solid' } })\n\
                    function farm_size() return 4, 3 end\n\
                    function snapshot_farm(m)\n\
                      local w, h = farm_size()\n\
                      local st = { version = 1, tiles = {}, solid = {}, meta = {} }\n\
                      for y = 0, h - 1 do\n\
                        for x = 0, w - 1 do\n\
                          local i = y * w + x + 1\n\
                          st.tiles[i] = tilemap.get_cell(m, x, y)\n\
                          st.solid[i] = tilemap.is_solid(m, x, y)\n\
                        end\n\
                      end\n\
                      for y = 0, h - 1 do\n\
                        for x = 0, w - 1 do\n\
                          local bag = {}\n\
                          for _, k in ipairs({ 'tilled', 'crop', 'state' }) do\n\
                            local v = tilemap.get_meta(m, x, y, k)\n\
                            if v then bag[k] = v end\n\
                          end\n\
                          if next(bag) then st.meta[x .. ',' .. y] = bag end\n\
                        end\n\
                      end\n\
                      return st\n\
                    end\n\
                    function restore_farm(m, st)\n\
                      local w, h = farm_size()\n\
                      for y = 0, h - 1 do\n\
                        for x = 0, w - 1 do\n\
                          local i = y * w + x + 1\n\
                          tilemap.set_cell(m, x, y, st.tiles[i])\n\
                          tilemap.set_solid(m, x, y, st.solid[i])\n\
                        end\n\
                      end\n\
                      for cell, bag in pairs(st.meta or {}) do\n\
                        local x, y = cell:match('^(%d+),(%d+)$')\n\
                        assert(x and y, 'bad meta cell ' .. tostring(cell))\n\
                        for k, v in pairs(bag) do\n\
                          tilemap.set_meta(m, tonumber(x), tonumber(y), k, v)\n\
                        end\n\
                      end\n\
                    end\n\
                    function assert_farm_restored(m)\n\
                      assert(tilemap.get_cell(m, 1, 2) == 3, 'tile 1,2')\n\
                      assert(tilemap.is_solid(m, 1, 2) == true, 'solid 1,2')\n\
                      assert(tilemap.get_meta(m, 0, 0, 'tilled') == 'yes', 'tilled 0,0')\n\
                      assert(tilemap.get_meta(m, 2, 1, 'crop') == 'tomato', 'crop 2,1')\n\
                      assert(tilemap.get_meta(m, 1, 1, 'tilled') == nil, 'no stray meta')\n\
                    end\n\
                    function update() return 'ok' end";

        // ---- first session: farm -> edit -> store -> home -> farm -> restore.
        // 启动脚本只加载一次；后续步骤通过 `execute_console` 在同一 VM 中执行，
        // 因为 `execute_project_script` 会重建 Lua VM，不能依赖跨脚本的全局 helper。
        // 切图不会重置 VM，因此 snapshot/restore helper 会跨 map.go 保留。
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        runtime.set_save_path(save_path.clone());
        let path = script(boot);
        runtime.execute_project_script(&path).unwrap();

        runtime
            .execute_console("assert(map.go('farm', 5, 5))")
            .unwrap();
        runtime.process_scene_transition().unwrap();
        runtime
            .execute_console(
                "local m = assert(entity.find('FarmMap'), 'FarmMap')\n\
                 tilemap.set_cell(m, 1, 2, 3)\n\
                 tilemap.set_solid(m, 1, 2, true)\n\
                 tilemap.set_meta(m, 0, 0, 'tilled', 'yes')\n\
                 tilemap.set_meta(m, 2, 1, 'crop', 'tomato')\n\
                 assert(map.state('farm', snapshot_farm(m)), 'store farm state')\n\
                 local st = map.state('farm')\n\
                 assert(st.version == 1, 'version')\n\
                 assert(st.tiles[10] == 3, 'tiles[10]')\n\
                 assert(st.solid[10] == true, 'solid[10]')\n\
                 assert(st.meta['0,0'].tilled == 'yes', 'meta 0,0')\n",
            )
            .unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        runtime
            .execute_console("assert(map.go('home', 5, 5))")
            .unwrap();
        runtime.process_scene_transition().unwrap();
        runtime
            .execute_console("assert(map.go('farm', 5, 5))")
            .unwrap();
        runtime.process_scene_transition().unwrap();
        // 切回地图时场景中的初始值优先；必须重新应用快照，才能保证
        // tile/meta/solid 与持久化状态一致。
        runtime
            .execute_console(
                "local m = assert(entity.find('FarmMap'), 'FarmMap')\n\
                 assert(tilemap.get_cell(m, 1, 2) == 0, 'scene baseline on re-entry')\n\
                 local st = assert(map.state('farm'), 'state survives scene change')\n\
                 restore_farm(m, st)\n\
                 assert_farm_restored(m)\n\
                 assert(save.commit(), 'commit')\n",
            )
            .unwrap();
        runtime.call_update().unwrap();
        assert!(save_path.is_file(), "session one should commit a save file");

        // ---- second session: fresh runtime boots on the same project/save.
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        runtime.set_save_path(save_path.clone());
        let boot_path = script(boot);
        runtime.execute_project_script(&boot_path).unwrap();
        runtime
            .execute_console(
                "assert(save.load(), 'load save')\n\
                 local st = map.state('farm')\n\
                 assert(st ~= nil and st.version == 1, 'restart state present')\n\
                 assert(st.tiles[10] == 3, 'restart tiles')\n\
                 assert(map.go('farm', 5, 5))\n",
            )
            .unwrap();
        runtime.process_scene_transition().unwrap();
        runtime
            .execute_console(
                "local m = assert(entity.find('FarmMap'), 'FarmMap')\n\
                 restore_farm(m, assert(map.state('farm'), 'restart state'))\n\
                 assert_farm_restored(m)\n",
            )
            .unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");

        let _ = fs::remove_file(path);
        let _ = fs::remove_file(boot_path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn data_schema_registration_rejects_invalid_tables_on_load() {
        let root = std::env::temp_dir().join(format!("p2-data-schema-{}", std::process::id()));
        let data = root.join("data");
        fs::create_dir_all(&data).unwrap();
        fs::write(
            data.join("items.json"),
            r#"{"tomato":{"name":"Tomato","stack":8}}"#,
        )
        .unwrap();
        fs::write(data.join("bad.json"), r#"{"tomato":{"stack":"big"}}"#).unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        let path = script(
            "local items_schema = { ['$kind'] = 'map', values = { name = 'string', stack = 'integer' } }\n\
             data.schema('data/items.json', items_schema)\n\
             data.schema('data/bad.json', items_schema)\n\
             local ok = data.load('data/items.json')\n\
             assert(ok.tomato.name == 'Tomato')\n\
             assert(not pcall(function() data.load('data/bad.json') end))\n\
             local err = select(2, pcall(function() data.load('data/bad.json') end))\n\
             assert(tostring(err):find('schema violation'), err)\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(path);
    }

    /// 「开始新游戏」必须能丢弃引擎侧持久键：`save.clear()` 清空全部内存键，
    /// `map.state_clear()` 只清 `__mapstate:*`（可指定单个状态名）。
    /// 这些键不属于任何游戏侧 provider，残留会让新档读回上一局的地图/农田/建筑。
    #[test]
    fn save_clear_and_map_state_clear_drop_persisted_keys() {
        let root = std::env::temp_dir().join(format!("p2-state-clear-{}", std::process::id()));
        let dir = root.join(".data");
        fs::create_dir_all(&dir).unwrap();
        let save_path = dir.join("save.sav");

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_save_path(save_path.clone());
        let path = script(
            "save.write('hp', '75')\n\
             local cleared = save.clear()\n\
             assert(cleared >= 1, 'clear should report removed keys: ' .. tostring(cleared))\n\
             assert(save.read('hp') == nil, 'keys are gone after clear')\n\
             assert(map.state('farm', { tiles = { 1, 2 } }))\n\
             assert(map.state('buildings', { version = 1, list = {} }))\n\
             local removed = map.state_clear()\n\
             assert(removed >= 2, 'state_clear removes every __mapstate key: ' .. tostring(removed))\n\
             assert(map.state('farm') == nil, 'farm state cleared')\n\
             assert(map.state('buildings') == nil, 'buildings state cleared')\n\
             assert(map.state('single', { x = 1 }))\n\
             map.state_clear('single')\n\
             assert(map.state('single') == nil, 'named clear removes that state')\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(path);
    }

    /// 新档清理的端到端语义：写档 → 读回 → `save.clear()` → 再次读回为空，
    /// 且地图/出生点键同样被丢弃（`map.current()` 回落到当前场景）。
    #[test]
    fn new_game_style_clear_removes_map_keys_and_survives_reload() {
        let root = std::env::temp_dir().join(format!("p2-newgame-clear-{}", std::process::id()));
        let dir = root.join(".data");
        fs::create_dir_all(&dir).unwrap();
        let save_path = dir.join("save.sav");

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_save_path(save_path.clone());
        let writer = script(
            "map.set_spawn(120, 240)\n\
             save.write('hp', '75')\n\
             assert(save.commit())\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&writer).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        assert!(save_path.is_file());

        // 第二次会话：读档 → 清档 → 落盘 → 第三次会话读回：什么都不该剩下。
        let mut fresh = LuaRuntime::new().unwrap();
        fresh.set_save_path(save_path.clone());
        let clearer = script(
            "local ok = save.load()\n\
             assert(ok, 'save should load')\n\
             assert(save.read('hp') == '75')\n\
             assert(map.spawn() ~= nil, 'spawn persisted')\n\
             save.clear()\n\
             map.state_clear()\n\
             assert(save.read('hp') == nil)\n\
             assert(map.spawn() == nil, 'spawn key dropped by clear')\n\
             assert(save.commit())\n\
             function update() return 'ok' end",
        );
        fresh.execute_project_script(&clearer).unwrap();
        assert_eq!(fresh.call_update().unwrap(), "ok");

        let mut third = LuaRuntime::new().unwrap();
        third.set_save_path(save_path.clone());
        let reader = script(
            "local ok = save.load()\n\
             assert(ok, 'cleared save should still be a valid file')\n\
             assert(save.read('hp') == nil, 'no keys survive a cleared new game')\n\
             assert(map.spawn() == nil, 'no spawn survives a cleared new game')\n\
             function update() return 'ok' end",
        );
        third.execute_project_script(&reader).unwrap();
        assert_eq!(third.call_update().unwrap(), "ok");

        let _ = fs::remove_dir_all(root);
        for path in [writer, clearer, reader] {
            let _ = fs::remove_file(path);
        }
    }

    #[test]
    fn save_load_rejects_older_save() {
        let root =
            std::env::temp_dir().join(format!("p2-save-version-reject-{}", std::process::id()));
        let dir = root.join(".data");
        fs::create_dir_all(&dir).unwrap();
        let save_path = dir.join("save.sav");
        fs::write(&save_path, "save_version=0\nhp\t10\n").unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_save_path(save_path.clone());
        let path = script("assert(not save.load())\nfunction update() return 'ok' end");
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(path);
    }

    /// 回归（P0）：多行/含制表符的值曾经写出伪行，使**整个存档**在下次 load
    /// 时无法解析。现在 `save.write` + `commit` + 新会话 `save.load` 必须无损往返。
    #[test]
    fn save_write_round_trips_multiline_and_cjk_values_across_sessions() {
        let root = std::env::temp_dir().join(format!("p2-save-escape-{}", std::process::id()));
        let dir = root.join(".data");
        fs::create_dir_all(&dir).unwrap();
        let save_path = dir.join("save.sav");

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_save_path(save_path.clone());
        let writer = script(
            "save.write('hp', '75')\n\
             save.write('note', 'line one\\nline two')\n\
             save.write('tabbed', 'a\\tb')\n\
             save.write('草药', '灵芝 x2')\n\
             assert(save.commit(), 'commit')\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&writer).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        assert!(save_path.is_file(), "save file should exist after commit");

        // 磁盘上每个键值对只占一行（1 行版本头 + 4 行数据）。
        let text = fs::read_to_string(&save_path).unwrap();
        assert_eq!(text.lines().count(), 5, "unexpected save text: {text:?}");
        assert!(text.starts_with("save_version=2\n"));

        let mut fresh = LuaRuntime::new().unwrap();
        fresh.set_save_path(save_path.clone());
        let reader = script(
            "local ok, reason = save.load()\n\
             assert(ok, tostring(reason))\n\
             assert(save.read('hp') == '75', 'hp')\n\
             assert(save.read('note') == 'line one\\nline two', 'note')\n\
             assert(save.read('tabbed') == 'a\\tb', 'tabbed')\n\
             assert(save.read('草药') == '灵芝 x2', 'cjk')\n\
             function update() return 'ok' end",
        );
        fresh.execute_project_script(&reader).unwrap();
        assert_eq!(fresh.call_update().unwrap(), "ok");

        let _ = fs::remove_dir_all(root);
        for path in [writer, reader] {
            let _ = fs::remove_file(path);
        }
    }

    /// `save.load` 必须区分「没有存档」与「存档已损坏」：两者都返回 false，
    /// 但第二个返回值给出原因，诊断面板据此才能告诉玩家进度是丢了还是从没存过。
    #[test]
    fn save_load_reports_missing_versus_corrupt() {
        let root = std::env::temp_dir().join(format!("p2-save-reason-{}", std::process::id()));
        let dir = root.join(".data");
        fs::create_dir_all(&dir).unwrap();
        let save_path = dir.join("save.sav");

        // 1) 文件不存在 → "missing"
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_save_path(save_path.clone());
        let missing = script(
            "local ok, reason = save.load()\n\
             assert(ok == false)\n\
             assert(reason == 'missing', tostring(reason))\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&missing).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");

        // 2) 损坏文件 → "corrupt: ..."，且原文件不被覆盖
        let corrupt_text = "save_version=1\nhp\t75\nbroken row\n";
        fs::write(&save_path, corrupt_text).unwrap();
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_save_path(save_path.clone());
        let corrupt = script(
            "local ok, reason = save.load()\n\
             assert(ok == false)\n\
             assert(reason:sub(1, 8) == 'corrupt:', tostring(reason))\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&corrupt).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        assert_eq!(
            fs::read_to_string(&save_path).unwrap(),
            corrupt_text,
            "损坏存档必须原样保留，绝不能被覆盖"
        );

        let _ = fs::remove_dir_all(root);
        for path in [missing, corrupt] {
            let _ = fs::remove_file(path);
        }
    }

    /// `save.write` 必须拒绝空键：它可以被写进内存，但会让整份存档在下次
    /// `load` 时解析失败（等于一次误用毁掉玩家进度）。
    #[test]
    fn save_write_rejects_empty_key() {
        let mut runtime = LuaRuntime::new().unwrap();
        let path = script(
            "assert(not pcall(function() save.write('', 'x') end))\n\
             local err = select(2, pcall(function() save.write('', 'x') end))\n\
             assert(tostring(err):find('must not be empty'), tostring(err))\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_file(path);
    }

    /// 回归：v1 旧存档（裸行）仍然可读，且读入后能正常 commit 成 v2。
    #[test]
    fn legacy_v1_save_still_loads_and_upgrades_on_commit() {
        let root = std::env::temp_dir().join(format!("p2-save-v1-{}", std::process::id()));
        let dir = root.join(".data");
        fs::create_dir_all(&dir).unwrap();
        let save_path = dir.join("save.sav");
        fs::write(&save_path, "save_version=1\nhp\t75\nscene\tarena\n").unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_save_path(save_path.clone());
        let path = script(
            "local ok, reason = save.load()\n\
             assert(ok, tostring(reason))\n\
             assert(save.read('hp') == '75')\n\
             assert(save.read('scene') == 'arena')\n\
             save.write('note', 'line one\\nline two')\n\
             assert(save.commit())\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let upgraded = fs::read_to_string(&save_path).unwrap();
        assert!(
            upgraded.starts_with("save_version=2\n"),
            "读入 v1 后应写成 v2: {upgraded:?}"
        );
        assert_eq!(
            crate::core::save::decode(&upgraded).unwrap().get("hp"),
            Some(&"75".to_owned())
        );

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn calendar_and_day_lifecycle_lua_api() {
        let mut runtime = LuaRuntime::new().unwrap();
        let path = script(
            "local now = calendar.now()\n\
             assert(now.absolute_day == 1 and now.day == 1 and now.season == 1 and now.year == 1)\n\
             calendar.set(1, 2, 1, 60)\n\
             local spring2 = calendar.now()\n\
             assert(spring2.season == 2 and spring2.day == 1 and spring2.absolute_day == 29)\n\
             ended = 0\n\
             started = 0\n\
             function on_day_end() ended = ended + 1 end\n\
             function on_day_start() started = started + 1 end\n\
             local d = day.sleep()\n\
             assert(d == 30 and ended == 1 and started == 1)\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn ui_panels_images_grids_and_drag_lua_api() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_ui_viewport(1280, 720);
        let path = script(
            "ui.panel('bg', 0, 0, 200, 200, 0.1, 0.1, 0.2, 1)\n\
             ui.image('icon', 10, 10, 32, 32, 'assets/tomato.png', 1, 0, 0)\n\
             ui.grid('bag', 10, 60, 4, 2, 40, 4)\n\
             ui.grid_cell('bag', 1, 'assets/tomato.png', 1, 0, 0, 1)\n\
             assert(ui.grid_hit('bag', 20, 70) == 1)\n\
             assert(ui.grid_hit('bag', 400, 70) == nil)\n\
             ui.modal('shop', 300, 100, 220, 140, 'Market')\n\
             assert(ui.drag_begin('bag', 1))\n\
             assert(ui.drag_active().index == 1)\n\
             function on_drag(from_id, from_index, to_id, to_index)\n\
               dragged = from_id .. ':' .. from_index .. ':' .. to_id .. ':' .. to_index\n\
             end\n\
             assert(ui.drag_end('bag', 2))\n\
             assert(dragged == 'bag:1:bag:2')\n\
             ui.tooltip('tip', 20, 280, 'Hoe', 120, 28)\n\
             ui.modal_hide('shop')\n\
             local icon = ui.image_get('icon')\n\
             assert(icon.texture == 'assets/tomato.png')\n\
             assert(icon.x == 10 and icon.y == 10 and icon.width == 32 and icon.height == 32)\n\
             assert(icon.r == 1 and icon.g == 0 and icon.b == 0)\n\
             assert(ui.image_get('missing') == nil)\n\
             ui.image('tmp', 0, 0, 8, 8, 'assets/tomato.png', 1, 1, 1)\n\
             ui.clear('tmp')\n\
             assert(ui.image_get('tmp') == nil)\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let dynamic = runtime.with_dynamic_ui(|panels, images, grids| {
            (panels.clone(), images.clone(), grids.clone())
        });
        assert!(dynamic.0.contains_key("bg"));
        assert!(dynamic.1.contains_key("icon"));
        assert!(dynamic.2.contains_key("bag"));
        assert_eq!(dynamic.2["bag"].cells.len(), 8);
        let _ = fs::remove_file(path);
    }

    /// `ui.measure_text` / `ui.text_get`（M30）：中文排版的度量与读回。
    #[test]
    fn measure_text_and_text_get_lua_api() {
        let root = std::env::temp_dir().join(format!("p2-measure-{}", std::process::id()));
        let fonts = root.join("assets").join("fonts");
        fs::create_dir_all(&fonts).unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../editor/fonts/NotoSansSC-VF.ttf");
        fs::copy(&fixture, fonts.join("NotoSansSC-VF.ttf")).unwrap();
        fs::write(
            root.join("project.toml"),
            "font = \"assets/fonts/NotoSansSC-VF.ttf\"\n",
        )
        .unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        let path = script(
            "
             local cjk = ui.measure_text('江湖', 15)
             assert(cjk.missing == '' and cjk.missing_count == 0)
             assert(cjk.glyphs == 2 and cjk.lines == 1)
             assert(cjk.width > 0 and cjk.height > 0)
             local ascii = ui.measure_text('AB', 15)
             -- CJK 满宽、ASCII 半宽：这正是按字数裁剪会算错的地方。
             assert(ascii.width < cjk.width)
             local long = ui.measure_text('江湖', 30)
             assert(long.width > cjk.width * 1.9 and long.width < cjk.width * 2.1)
             local two = ui.measure_text('江湖\\n有田', 15)
             assert(two.lines == 2 and two.height > cjk.height)
             local emoji = ui.measure_text('江湖🙂', 15)
             assert(emoji.missing_count == 1 and emoji.missing == '🙂')
             assert(not pcall(function() return ui.measure_text('x', 0) end))
             assert(not pcall(function() return ui.measure_text(nil, 15) end))
             ui.text_at('hud:line', 12, 30, '江湖有田', 1.0, 15)
             ui.text_at('hud:scaled', 12, 60, '江湖', 2.0)
             local line = ui.text_get('hud:line')
             assert(line.text == '江湖有田' and line.x == 12 and line.y == 30)
             assert(line.size == 15 and line.scale == 1.0 and line.visible == true)
             -- 未给 font_size 时按渲染口径回落到 scale × EM_PX(12)。
             assert(ui.text_get('hud:scaled').size == 24)
             assert(ui.text_get('missing') == nil)
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn save_commit_load_and_scene_transition() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = std::env::temp_dir().join(format!("p2-save-scene-{}", std::process::id()));
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).unwrap();
        let mut arena = Scene::new("arena");
        arena.add_entity(SceneEntityId(1), "Player", None).unwrap();
        arena
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
            );
        arena
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Sprite".into(),
                "texture=assets/player.png;size=32x32".into(),
            );
        arena.save(scenes.join("arena.scene")).unwrap();

        // save + commit to disk
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        runtime.set_save_path(root.join(".data").join("save.sav"));
        let first = script(
            "save.write('hp', '75')\nsave.write('scene', 'arena')\nassert(save.commit())\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&first).unwrap();
        runtime.call_update().unwrap();
        assert!(root.join(".data").join("save.sav").is_file());

        // a fresh runtime loads the save back
        let mut fresh = LuaRuntime::new().unwrap();
        fresh.set_save_path(root.join(".data").join("save.sav"));
        let load_script = script(
            "assert(save.load())\nassert(save.read('hp') == '75')\nfunction update() return 'ok' end",
        );
        fresh.execute_project_script(&load_script).unwrap();
        fresh.call_update().unwrap();

        // scene.load queues a transition; process_scene_transition applies it
        let switch = script("assert(scene.load('arena'))\nfunction update() return 'ok' end");
        runtime.execute_project_script(&switch).unwrap();
        runtime.process_scene_transition().unwrap();
        let players = runtime.with_world(|world| world.entities_with::<Sprite>());
        assert_eq!(players.len(), 1, "arena player instantiated after switch");
        assert_eq!(runtime.with_world(|world| world.entity_count()), 1);
        // Lua can find the freshly-instantiated player after the transition
        let check = script(
            "local p = entity.find('Player')\nassert(p ~= nil)\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&check).unwrap();
        runtime.call_update().unwrap();

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(first);
        let _ = fs::remove_file(load_script);
        let _ = fs::remove_file(switch);
        let _ = fs::remove_file(check);
    }

    #[test]
    fn persistent_entities_survive_scene_transition() {
        use crate::core::scene::{Scene, SceneEntityId};
        use crate::core::ui::UiText;

        let root = std::env::temp_dir().join(format!("p2-persist-{}", std::process::id()));
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).unwrap();

        let mut arena = Scene::new("arena");
        arena.add_entity(SceneEntityId(1), "Player", None).unwrap();
        arena
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Sprite".into(),
                "texture=assets/player.png;size=32x32".into(),
            );
        arena.add_entity(SceneEntityId(2), "Hud", None).unwrap();
        let hud = arena.entities.get_mut(&SceneEntityId(2)).unwrap();
        hud.components.insert(
            "Text".into(),
            "text=Score:0;font_size=24;color=1,1,1".into(),
        );
        hud.components.insert(
            "UiLayout".into(),
            "anchors=0,0,0,0;offsets=10,10,10,10".into(),
        );
        hud.metadata.insert("persistent".into(), "true".into());
        arena.save(scenes.join("arena.scene")).unwrap();

        let mut hub = Scene::new("hub");
        hub.add_entity(SceneEntityId(1), "Boss", None).unwrap();
        hub.entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Sprite".into(), "texture=assets/boss.png;size=32x32".into());
        hub.save(scenes.join("hub.scene")).unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        let load_arena = script("assert(scene.load('arena'))\nfunction update() return 'ok' end");
        runtime.execute_project_script(&load_arena).unwrap();
        runtime.process_scene_transition().unwrap();
        let load_hub = script("assert(scene.load('hub'))\nfunction update() return 'ok' end");
        runtime.execute_project_script(&load_hub).unwrap();
        runtime.process_scene_transition().unwrap();

        // Hud (persistent, UI text) survives; arena Player gone; hub Boss present.
        let ui_texts: Vec<String> = runtime.with_world(|world| {
            world
                .entities_with::<UiText>()
                .into_iter()
                .filter_map(|id| {
                    world
                        .get::<UiText>(id)
                        .ok()
                        .flatten()
                        .map(|t| t.text.clone())
                })
                .collect()
        });
        assert_eq!(ui_texts.len(), 1, "persistent Hud text survives");
        assert_eq!(ui_texts[0], "Score:0");
        let sprites = runtime.with_world(|world| world.entities_with::<Sprite>());
        assert_eq!(sprites.len(), 1, "only hub Boss sprite remains");
        assert_eq!(runtime.with_world(|world| world.entity_count()), 2);
        let check = script(
            "local hud = entity.find('Hud')\nassert(hud ~= nil)\nlocal boss = entity.find('Boss')\nassert(boss ~= nil)\nassert(entity.find('Player') == nil)\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&check).unwrap();
        runtime.call_update().unwrap();

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(load_arena);
        let _ = fs::remove_file(load_hub);
        let _ = fs::remove_file(check);
    }

    #[test]
    fn persistent_duplicate_name_new_scene_wins() {
        use crate::core::scene::{Scene, SceneEntityId};
        use crate::core::ui::UiText;

        let root = std::env::temp_dir().join(format!("p2-persist-name-{}", std::process::id()));
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).unwrap();

        let mut arena = Scene::new("arena");
        arena.add_entity(SceneEntityId(1), "Hud", None).unwrap();
        let hud = arena.entities.get_mut(&SceneEntityId(1)).unwrap();
        hud.components.insert(
            "Text".into(),
            "text=Score:0;font_size=24;color=1,1,1".into(),
        );
        hud.components.insert(
            "UiLayout".into(),
            "anchors=0,0,0,0;offsets=10,10,10,10".into(),
        );
        hud.metadata.insert("persistent".into(), "true".into());
        arena.save(scenes.join("arena.scene")).unwrap();

        let mut hub = Scene::new("hub");
        hub.add_entity(SceneEntityId(1), "Hud", None).unwrap();
        let hud = hub.entities.get_mut(&SceneEntityId(1)).unwrap();
        hud.components.insert(
            "Text".into(),
            "text=Score:99;font_size=24;color=1,1,1".into(),
        );
        hud.components.insert(
            "UiLayout".into(),
            "anchors=0,0,0,0;offsets=10,10,10,10".into(),
        );
        hud.metadata.insert("persistent".into(), "true".into());
        hub.save(scenes.join("hub.scene")).unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        let load_arena = script("scene.load('arena')\nfunction update() return 'ok' end");
        runtime.execute_project_script(&load_arena).unwrap();
        runtime.process_scene_transition().unwrap();
        let load_hub = script("scene.load('hub')\nfunction update() return 'ok' end");
        runtime.execute_project_script(&load_hub).unwrap();
        runtime.process_scene_transition().unwrap();

        let ui_texts: Vec<String> = runtime.with_world(|world| {
            world
                .entities_with::<UiText>()
                .into_iter()
                .filter_map(|id| {
                    world
                        .get::<UiText>(id)
                        .ok()
                        .flatten()
                        .map(|t| t.text.clone())
                })
                .collect()
        });
        assert_eq!(ui_texts.len(), 1, "exactly one Hud");
        assert_eq!(ui_texts[0], "Score:99", "new scene's persistent Hud wins");

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(load_arena);
        let _ = fs::remove_file(load_hub);
    }

    #[test]
    fn click_event_dispatches_lua_on_click() {
        use crate::core::input::MouseButton;
        use crate::core::scene::{Scene, SceneEntityId};

        let root = std::env::temp_dir().join(format!("p2-click-{}", std::process::id()));
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).unwrap();
        let mut menu = Scene::new("menu");
        menu.add_entity(SceneEntityId(1), "Start", None).unwrap();
        menu.entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "UiEvents".into(),
                "event=click;function=on_click".to_owned(),
            );
        menu.save(scenes.join("menu.scene")).unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        let load = script("scene.load('menu')\nfunction update() return 'ok' end");
        runtime.execute_project_script(&load).unwrap();
        runtime.process_scene_transition().unwrap();

        // 定义全局 on_click + 检查用 update（同一脚本，VM 不替换）。
        let click = script(
            "function on_click(name) clicked = name end\nfunction update() return tostring(clicked or 'nil') end",
        );
        runtime.execute_project_script(&click).unwrap();
        let mut input = InputSnapshot::default();
        input.cursor_moved(50.0, 20.0);
        input.mouse_event(MouseButton::Left, true);
        // 命中矩形由运行时 compute_click_targets 预计算；此处直接传入覆盖点击点的目标。
        runtime
            .process_ui_clicks(
                &input,
                &[UiClickTarget {
                    name: "Start".to_owned(),
                    origin: [0.0, 0.0],
                    size: [1280.0, 720.0],
                    functions: vec!["on_click".to_owned()],
                }],
            )
            .unwrap();

        let result = runtime.call_update().unwrap();
        assert_eq!(
            result, "Start",
            "on_click('Start') fired for the structured UiEvents entity"
        );

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(load);
        let _ = fs::remove_file(click);
    }

    #[test]
    fn production_sample_main_script_runs_menu_update() {
        use crate::core::scene::Scene;
        use crate::core::ui_events::UiEvents;

        let root = production_sample_root();
        if !root.join("scenes/menu.scene").is_file() {
            return;
        }
        let scene = Scene::load(root.join("scenes/menu.scene")).expect("load sample menu");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 7);
        runtime
            .execute_project_script(&root.join("scripts/showcase.lua"))
            .expect("load sample showcase.lua");

        assert_eq!(runtime.call_update().unwrap(), "menu_ready");
        // Button has been removed; menu buttons use structured UiEvents.
        let clicks = runtime.with_world(|world| world.entities_with::<UiEvents>().len());
        assert_eq!(clicks, 2, "menu scene should expose two UiEvents buttons");
    }

    #[test]
    fn production_sample_start_button_loads_production_scene() {
        use crate::core::{
            animation::AnimationStateMachine, input::MouseButton, scene::Scene, tilemap::Tilemap,
        };

        let root = production_sample_root();
        if !root.join("scenes/menu.scene").is_file() {
            return;
        }
        let scene = Scene::load(root.join("scenes/menu.scene")).expect("load sample menu");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        runtime.instantiate_scene(&scene).unwrap();
        runtime
            .execute_project_script(&root.join("scripts/showcase.lua"))
            .expect("load sample showcase.lua");

        let mut input = InputSnapshot::default();
        input.cursor_moved(520.0, 400.0);
        input.mouse_event(MouseButton::Left, true);
        // StartButton 命中矩形（覆盖点击点）→ on_click("StartButton") → 载入生产场景。
        runtime
            .process_ui_clicks(
                &input,
                &[UiClickTarget {
                    name: "StartButton".to_owned(),
                    origin: [300.0, 300.0],
                    size: [500.0, 200.0],
                    functions: vec!["on_click".to_owned()],
                }],
            )
            .unwrap();
        runtime.process_scene_transition().unwrap();

        let result = runtime.call_update().unwrap();
        assert!(
            result.starts_with("production_ok"),
            "production gameplay update should run after transition: {result}"
        );
        let counts = runtime.with_world(|world| {
            (
                world.entities_with::<Tilemap>().len(),
                world.entities_with::<AnimationStateMachine>().len(),
            )
        });
        assert_eq!(counts.0, 1, "production scene should load one tilemap");
        assert_eq!(
            counts.1, 11,
            "production scene should load player plus ten animated enemies"
        );
    }

    #[test]
    fn production_sample_can_save_and_return_to_menu() {
        use crate::core::{input::KeyCode, scene::Scene};

        let root = production_sample_root();
        if !root.join("scenes/menu.scene").is_file() {
            return;
        }
        let save_path = std::env::temp_dir().join(format!(
            "p2-production-sample-save-{}.sav",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be valid")
                .as_nanos()
        ));
        let _ = fs::remove_file(&save_path);
        let scene = Scene::load(root.join("scenes/menu.scene")).expect("load sample menu");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        runtime.set_save_path(save_path.clone());
        runtime.instantiate_scene(&scene).unwrap();
        runtime
            .execute_project_script(&root.join("scripts/showcase.lua"))
            .expect("load sample showcase.lua");

        let mut click = InputSnapshot::default();
        click.cursor_moved(520.0, 400.0);
        click.mouse_event(crate::core::input::MouseButton::Left, true);
        runtime
            .process_ui_clicks(
                &click,
                &[UiClickTarget {
                    name: "StartButton".to_owned(),
                    origin: [300.0, 300.0],
                    size: [500.0, 200.0],
                    functions: vec!["on_click".to_owned()],
                }],
            )
            .unwrap();
        runtime.process_scene_transition().unwrap();

        let mut save_input = InputSnapshot::default();
        save_input.key_event(KeyCode::F1, true);
        runtime.set_input(save_input);
        let save_result = runtime.call_update().unwrap();
        assert!(
            save_result.starts_with("production_ok"),
            "save frame should continue gameplay update: {save_result}"
        );
        assert!(save_path.is_file(), "F1 should commit a save file");

        let mut menu_input = InputSnapshot::default();
        menu_input.key_event(KeyCode::F4, true);
        runtime.set_input(menu_input);
        let switch_result = runtime.call_update().unwrap();
        assert_eq!(switch_result, "return_menu");
        runtime.process_scene_transition().unwrap();
        runtime.set_input(InputSnapshot::default());
        assert_eq!(runtime.call_update().unwrap(), "menu_ready");
        let clicks = runtime.with_world(|world| {
            world
                .entities_with::<crate::core::ui_events::UiEvents>()
                .len()
        });
        assert_eq!(
            clicks, 2,
            "menu scene should restore two UiEvents buttons after F4"
        );

        let _ = fs::remove_file(save_path);
    }

    #[test]
    fn lua_3d_api_moves_mesh_and_switches_camera() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("3d");
        scene.add_entity(SceneEntityId(1), "Camera", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Camera3D".into(),
                "position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100".into(),
            );
        scene.add_entity(SceneEntityId(2), "Sun", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Light".into(),
                "direction=0.4,0.8,0.5;color=1,1,1;intensity=1".into(),
            );
        scene.add_entity(SceneEntityId(3), "Hero", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(3))
            .unwrap()
            .components
            .insert(
                "Transform3D".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(3))
            .unwrap()
            .components
            .insert("MeshRenderer".into(), "mesh=assets/models/hero.glb".into());

        let mut runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 3);

        let path = script(
            "function update()\n\
             local h = entity.find('Hero')\n\
             assert(h ~= nil)\n\
             assert(mesh.get(h) == 'assets/models/hero.glb')\n\
             transform3d.move(h, 2, 3, 4)\n\
             local t = transform3d.get(h)\n\
             assert(t.x == 2 and t.y == 3 and t.z == 4)\n\
             local cam = entity.find('Camera')\n\
             assert(camera.get_active() == nil)\n\
             assert(camera.set_active(cam))\n\
             assert(camera.get_active() == cam)\n\
             local c = camera.get(cam)\n\
             assert(math.abs(c.fov - 90) < 0.001 and math.abs(c.near - 0.1) < 0.001 and c.far == 100)\n\
             local sun = entity.find('Sun')\n\
             light.set_active(sun)\n\
             light.set(sun, 0, 1, 0, 1, 0.5, 0.25, 2)\n\
             return '3d-ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "3d-ok");

        assert_eq!(
            runtime.active_camera(),
            Some(runtime.with_world(|world| world.entities_with::<Camera3D>()[0]))
        );
        assert_eq!(
            runtime.active_light(),
            Some(runtime.with_world(|world| world.entities_with::<Light>()[0]))
        );
        let moved = runtime.with_world(|world| {
            let hero = world.entities_with::<MeshRenderer>()[0];
            *world.get::<Transform3D>(hero).unwrap().unwrap()
        });
        assert_eq!(moved.position, Vec3::new(2.0, 3.0, 4.0));
        let light = runtime.with_world(|world| {
            let sun = world.entities_with::<Light>()[0];
            *world.get::<Light>(sun).unwrap().unwrap()
        });
        assert_eq!(light.direction, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(light.intensity, 2.0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn camera_set_moves_position_target_and_fov() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("cam");
        scene.add_entity(SceneEntityId(1), "Cam", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Camera3D".into(),
                "position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100".into(),
            );
        let mut runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

        let path = script(
            "function update()\n\
             local cam = entity.find('Cam')\n\
             assert(camera.set(cam, 1, 2, 3, 4, 5, 6, 45))\n\
             local c = camera.get(cam)\n\
             assert(c.x == 1 and c.y == 2 and c.z == 3)\n\
             assert(c.tx == 4 and c.ty == 5 and c.tz == 6)\n\
             assert(math.abs(c.fov - 45) < 0.001)\n\
             local bad = pcall(function() camera.set(cam, 0, 0, 0, 0, 0, 0, 0) end)\n\
             assert(not bad, 'fov=0 must be rejected')\n\
             return 'cam-ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "cam-ok");
        let cam = runtime.with_world(|world| {
            let id = world.entities_with::<Camera3D>()[0];
            *world.get::<Camera3D>(id).unwrap().unwrap()
        });
        assert_eq!(cam.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(cam.target, Vec3::new(4.0, 5.0, 6.0));
        assert!((cam.fov_y.to_degrees() - 45.0).abs() < 0.001);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn lua_3d_attach_and_validation_errors() {
        let mut runtime = LuaRuntime::new().unwrap();
        let path = script(
            "local e = entity.spawn('Cube')\n\
             transform3d.attach(e)\n\
             mesh.set(e, 'assets/models/cube.glb')\n\
             transform3d.set(e, 1, 2, 3)\n\
             local t = transform3d.get(e)\n\
             assert(t.x == 1 and t.y == 2 and t.z == 3)\n\
             assert(t.rw == 1 and t.sx == 1)\n\
             local ok = pcall(function() transform3d.set(e, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0) end)\n\
             assert(not ok, 'zero-scale must be rejected')\n\
             local bad = pcall(function() mesh.set(e, 'assets/models/cube.obj') end)\n\
             assert(not bad, 'non-glb mesh must be rejected')\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn lua_3d_animation_queries_and_switches_clips() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = crate::core::games_dir().join("examples").join("game_api");
        if !root.join("assets/models/m8-skin-quad.glb").is_file() {
            return;
        }
        let mut scene = Scene::new("3d-anim");
        scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Animation3D".into(),
                "mesh=assets/models/m8-skin-quad.glb;clip=Bend;looped=true".into(),
            );

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

        let path = script(
            "function update()\n\
             local h = entity.find('Hero')\n\
             assert(animation3d.get(h) == 'Bend')\n\
             local list = animation3d.list(h)\n\
             assert(#list == 1 and list[1] == 'Bend')\n\
             assert(not animation3d.set(h, 'Bend'))\n\
             assert(animation3d.time(h) >= 0)\n\
             assert(not animation3d.paused(h))\n\
             animation3d.set_paused(h, true)\n\
             assert(animation3d.paused(h))\n\
             return 'anim3d-ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "anim3d-ok");

        // The Animation3D component is present in the world and advancing works.
        let animations = runtime.with_world(|world| world.entities_with::<Animation3D>());
        assert_eq!(animations.len(), 1);
        runtime.with_world_mut(|world| {
            world
                .get_mut::<Animation3D>(animations[0])
                .unwrap()
                .unwrap()
                .advance(0.5);
        });
        assert!(
            runtime.with_world(|world| world
                .get::<Animation3D>(animations[0])
                .unwrap()
                .unwrap()
                .time())
                == 0.0,
            "paused animation must not advance"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn call_update_without_script_idles_instead_of_erroring() {
        // Engine-only diagnostic mode has no project script; update must be a
        // no-op rather than a fatal "nil to function" error (which used to
        // crash double-clicked runs).
        let runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.call_update().unwrap(), "");
    }

    #[test]
    fn invalid_entity_is_a_lua_error() {
        let path = script("transform.get(4294967297)");
        let mut runtime = LuaRuntime::new().unwrap();
        let error = runtime
            .execute_project_script(&path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid entity id"));
        let _ = fs::remove_file(path);
    }
    #[test]
    fn hot_reload_detects_changes_and_keeps_old_vm_on_failure() {
        let path = script("function update() return 'v1' end");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "v1");
        assert_eq!(
            runtime.reload_if_changed().unwrap(),
            ReloadStatus::Unchanged
        );
        fs::write(&path, "function update() return 'v2' end").unwrap();
        assert_eq!(runtime.reload_if_changed().unwrap(), ReloadStatus::Reloaded);
        assert_eq!(runtime.call_update().unwrap(), "v2");
        fs::write(&path, "function update(").unwrap();
        assert!(runtime.reload_if_changed().is_err());
        assert_eq!(runtime.call_update().unwrap(), "v2");
        let _ = fs::remove_file(path);
    }
    #[test]
    fn hot_reload_calls_lifecycle_callbacks() {
        let path = script(
            "function on_create() end; function on_destroy() end; function update() return 'v1' end",
        );
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        fs::write(&path, "function on_create() end; function on_destroy() end; function update() return 'v2' end").unwrap();
        runtime.reload_if_changed().unwrap();
        assert_eq!(runtime.call_update().unwrap(), "v2");
        let _ = fs::remove_file(path);
    }
    #[test]
    fn console_evaluates_values_and_updates_lua_state() {
        let mut runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.execute_console("return 1 + 2").unwrap(), "3");
        assert_eq!(runtime.execute_console("console_value = 7").unwrap(), "nil");
        assert_eq!(
            runtime.execute_console("return console_value").unwrap(),
            "7"
        );
    }

    /// M19-03：`eval_json` 覆盖表/数组/标量/nil 的转换与非法表达式错误，
    /// 且错误保留原始 Lua 消息（供编辑器原样显示）。
    #[test]
    fn eval_json_converts_tables_arrays_scalars_and_reports_errors() {
        let runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.eval_json("nil").unwrap(), serde_json::Value::Null);
        assert_eq!(runtime.eval_json("true").unwrap(), serde_json::json!(true));
        assert_eq!(runtime.eval_json("42").unwrap(), serde_json::json!(42));
        assert_eq!(runtime.eval_json("1.5").unwrap(), serde_json::json!(1.5));
        assert_eq!(
            runtime.eval_json("'text'").unwrap(),
            serde_json::json!("text")
        );
        // 纯数组表 → JSON 数组。
        assert_eq!(
            runtime.eval_json("return { 1, 2, 3 }").unwrap(),
            serde_json::json!([1, 2, 3])
        );
        // 字符串键表（含嵌套数组）→ JSON 对象。
        assert_eq!(
            runtime
                .eval_json("return { ok = true, issues = { 'a', 'b' } }")
                .unwrap(),
            serde_json::json!({ "ok": true, "issues": ["a", "b"] })
        );
        // 多语句块（编辑器域数据探针用 IIFE，但语句块本身也应可用）。
        assert_eq!(
            runtime
                .eval_json("local out = {}; for i = 1, 3 do out[i] = i * 2 end; return out")
                .unwrap(),
            serde_json::json!([2, 4, 6])
        );
        // 非法表达式：错误保留原始 Lua 语法消息。
        let error = runtime.eval_json("this is not lua").unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("eval_json"),
            "错误应带求值入口名: {message}"
        );
        assert!(
            message.contains("syntax error"),
            "错误应保留原始 Lua 消息: {message}"
        );
        // 运行期错误同样保留消息。
        let runtime_error = runtime.eval_json("error('probe boom')").unwrap_err();
        assert!(runtime_error.to_string().contains("probe boom"));
        // 不支持的 Lua 类型（函数）报错而不是 panic。
        assert!(runtime.eval_json("print").is_err());
    }

    #[test]
    fn console_errors_are_recorded_with_console_path() {
        let mut runtime = LuaRuntime::new().unwrap();
        assert!(runtime.execute_console("error('console boom')").is_err());
        let diagnostics = runtime.all_diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].path, PathBuf::from("<console>"));
        assert_eq!(diagnostics[0].phase, "runtime");
        assert!(diagnostics[0].message.contains("console boom"));
    }

    #[test]
    fn debugger_extracts_location_deduplicates_and_filters() {
        let path = script("function update() error('boom') end");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        assert!(runtime.call_update_debug().is_err());
        assert!(runtime.call_update_debug().is_err());
        assert_eq!(runtime.all_diagnostics().len(), 1);
        assert_eq!(runtime.all_diagnostics()[0].occurrences, 2);
        assert_eq!(runtime.all_diagnostics()[0].phase, "runtime");
        runtime.set_debug_filter(Some("does-not-match".to_string()));
        assert!(runtime.diagnostics().is_empty());
        runtime.set_debug_filter(Some("boom".to_string()));
        assert_eq!(runtime.diagnostics().len(), 1);
        runtime.clear_diagnostics();
        assert!(runtime.all_diagnostics().is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn debugger_extracts_line_from_lua_message() {
        let mut debugger = ScriptDebugger::new();
        debugger.record(PathBuf::from("game.lua"), "runtime", "game.lua:42: boom");
        assert_eq!(debugger.all()[0].line, Some(42));
        assert_eq!(debugger.all()[0].stack_trace, "game.lua:42: boom");
    }

    #[test]
    fn syntax_error_contains_path_and_line() {
        let path = script("function update(\n");
        let mut runtime = LuaRuntime::new().unwrap();
        let error = runtime
            .execute_project_script(&path)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&path.display().to_string()));
        let _ = fs::remove_file(path);
    }
    #[test]
    fn runtime_error_is_returned_without_panic() {
        let path = script("function update() error('boom') end");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        let error = runtime.call_update().unwrap_err().to_string();
        assert!(error.contains("boom"));
        assert!(error.contains(&path.display().to_string()));
        let _ = fs::remove_file(path);
    }

    /// M20-02：`audio` 表的总线 API——分组独立、主 × 分组、非法总线名报错。
    #[test]
    fn audio_bus_lua_api_groups_volumes_and_rejects_unknown_bus() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime
            .execute_console(
                "assert(#audio.buses() == 4)\n\
                 assert(audio.buses()[1] == 'master' and audio.buses()[4] == 'ui')\n\
                 audio.set_bus_volume('master', 0.5)\n\
                 audio.set_bus_volume('bgm', 0.4)\n\
                 audio.set_bus_volume('sfx', 0.8)\n\
                 audio.set_bus_volume('ui', 0.2)\n\
                 assert(math.abs(audio.bus_volume('master') - 0.5) < 1e-6)\n\
                 assert(math.abs(audio.bus_volume('bgm') - 0.4) < 1e-6)\n\
                 assert(math.abs(audio.bus_volume('sfx') - 0.8) < 1e-6)\n\
                 assert(math.abs(audio.bus_volume('ui') - 0.2) < 1e-6)\n\
                 assert(math.abs(audio.effective_volume('bgm') - 0.2) < 1e-6)\n\
                 assert(math.abs(audio.effective_volume('master') - 0.5) < 1e-6)\n\
                 assert(math.abs(audio.effective_volume('sfx') - 0.4) < 1e-6)\n\
                 do local ok = pcall(function() return audio.bus_volume('nope') end)\n\
                    assert(not ok, 'unknown bus must error') end\n\
                 do local ok = pcall(function() return audio.set_bus_volume('nope', 0.5) end)\n\
                    assert(not ok, 'unknown bus must error') end\n\
                 audio.set_bus_volume('bgm', 5.0)\n\
                 assert(math.abs(audio.bus_volume('bgm') - 1.0) < 1e-6)\n\
                 return 'ok'",
            )
            .unwrap();
    }

    /// M20-02：`audio.play_on`（总线播放）与 `settings_text`/`apply_settings` 往返。
    #[test]
    fn audio_lua_settings_text_round_trips_and_rejects_bad_version() {
        let mut runtime = LuaRuntime::new().unwrap();
        let text = runtime
            .execute_console(
                "audio.set_bus_volume('master', 0.6)\n\
                 audio.set_bus_volume('bgm', 0.3)\n\
                 audio.set_bus_volume('sfx', 0.9)\n\
                 audio.set_bus_volume('ui', 0.6)\n\
                 assert(select(1, audio.apply_settings('rustlua-audio-v0\\nbgm=0.9')) == false)\n\
                 assert(math.abs(audio.bus_volume('bgm') - 0.3) < 1e-6)\n\
                 return audio.settings_text()",
            )
            .unwrap();
        assert!(text.starts_with(crate::core::audio::AUDIO_SETTINGS_VERSION));
        assert!(text.contains("bgm=0.300"), "{text}");

        // 文本经 Lua 长字符串吃进第二个 VM，四路音量按总线路由恢复。
        let mut restored = LuaRuntime::new().unwrap();
        let expression = format!(
            "assert(select(1, audio.apply_settings([==[\n{text}]==])) == true)\n\
             assert(math.abs(audio.bus_volume('master') - 0.6) < 1e-6)\n\
             assert(math.abs(audio.bus_volume('bgm') - 0.3) < 1e-6)\n\
             assert(math.abs(audio.bus_volume('sfx') - 0.9) < 1e-6)\n\
             assert(math.abs(audio.bus_volume('ui') - 0.6) < 1e-6)\n\
             assert(math.abs(audio.effective_volume('bgm') - 0.18) < 1e-6)\n\
             return 'ok'"
        );
        assert_eq!(restored.execute_console(&expression).unwrap(), "ok");

        // 播放失败（无 asset_dir）返回 false + 原因，不抛错、不中断玩法。
        let (ok, why) = {
            let raw = restored
                .execute_console(
                    "local ok, why = audio.play_on('ui', '__definitely_missing__')\n\
                     return tostring(ok) .. '|' .. tostring(why ~= nil and #why > 0)",
                )
                .unwrap();
            let mut parts = raw.split('|');
            (
                parts.next().unwrap_or_default().to_owned(),
                parts.next().unwrap_or_default().to_owned(),
            )
        };
        assert_eq!(ok, "false");
        assert_eq!(why, "true");
    }

    /// M20-02：工程设置文件 `<project>/settings.toml` 的读写入口。
    #[test]
    fn audio_lua_project_settings_load_and_save() {
        let root = std::env::temp_dir().join(format!(
            "rustlua-lua-settings-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("settings.toml"),
            "# 说明\n[audio]\nmaster = 0.5\nbgm = 0.25\nsfx = 0.75\nui = 0.5\n\n[input]\nmap = \"keep\"\n",
        )
        .unwrap();
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        let path = runtime
            .execute_console("assert(audio.settings_path() ~= nil); return audio.settings_path()")
            .unwrap();
        assert!(path.ends_with("settings.toml"), "{path}");
        runtime
            .execute_console(
                "assert(select(1, audio.load_settings()) == true)\n\
                 assert(math.abs(audio.bus_volume('bgm') - 0.25) < 1e-6)\n\
                 assert(math.abs(audio.effective_volume('bgm') - 0.125) < 1e-6)\n\
                 audio.set_bus_volume('ui', 0.9)\n\
                 assert(select(1, audio.save_settings()) == true)\n\
                 return 'ok'",
            )
            .unwrap();
        let text = fs::read_to_string(root.join("settings.toml")).unwrap();
        assert!(text.contains("# 说明"), "{text}");
        assert!(text.contains("map = \"keep\""), "{text}");
        assert!(text.contains("ui = 0.9"), "{text}");
        // 缺失文件 → 默认值 + 明确返回 false, "missing"，不报错。
        let empty = root.join("empty");
        fs::create_dir_all(&empty).unwrap();
        let mut bare = LuaRuntime::new().unwrap();
        bare.set_project_root(empty.clone());
        assert_eq!(
            bare.execute_console(
                "local ok, why = audio.load_settings(); return tostring(ok) .. '|' .. why"
            )
            .unwrap(),
            "false|missing"
        );
        // 无工程根：明确失败而不是 panic。
        let mut rootless = LuaRuntime::new().unwrap();
        assert_eq!(
            rootless
                .execute_console("return select(2, audio.save_settings())")
                .unwrap(),
            "no_project_root"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn input_action_map_defaults_are_available_to_lua() {
        let mut runtime = LuaRuntime::new().unwrap();
        let raw = runtime
            .execute_console(
                "assert(#input.actions() >= 15, 'default actions must be registered')\n\
                 local text = input.action_map_text()\n\
                 assert(text:find('rustlua-actions-v2', 1, true) == 1)\n\
                 assert(text:find('interact=key:keye', 1, true) ~= nil)\n\
                 local list = input.bindings('confirm')\n\
                 assert(#list == 2 and list[1] == 'key:space' and list[2] == 'pad:0')\n\
                 assert(#input.bindings('missing_action') == 0)\n\
                 return tostring(#input.actions())",
            )
            .unwrap();
        assert!(raw.parse::<usize>().unwrap_or_default() >= 15, "{raw}");
        // 宿主读到的映射与脚本同源（同一份实例）。
        assert_eq!(
            runtime.action_map().bindings("interact"),
            [Binding::Key(KeyCode::KeyE)]
        );
        assert!(runtime.input_diagnostics().is_empty());
    }

    /// M20-07：键盘键位与抽象手柄按钮触发同一动作，结论一致；
    /// 摇杆给模拟量、键位给 0/1；原始手柄查询（`pad_*`）与动作层一致。
    #[test]
    fn input_action_api_matches_keyboard_and_gamepad_bindings() {
        let mut runtime = LuaRuntime::new().unwrap();
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyE, true);
        runtime.set_input(input);
        runtime
            .execute_console(
                "assert(input.action('interact') == true)\n\
                 assert(input.action_pressed('interact') == true)\n\
                 assert(input.action_released('interact') == false)\n\
                 assert(input.action('move_up') == false)\n\
                 assert(input.action_axis('move_left', 'move_right') == 0)\n\
                 assert(input.pad_button(0) == false)\n\
                 assert(input.pad_axis(1) == 0)\n\
                 return 'ok'",
            )
            .unwrap();
        // 同一动作改由手柄 A 键（confirm）触发：语义与键盘一致。
        let mut input = InputSnapshot::default();
        input.gamepad_button_event(0, true);
        runtime.set_input(input);
        let raw = runtime
            .execute_console(
                "assert(input.action('confirm') == true)\n\
                 assert(input.action_pressed('confirm') == true)\n\
                 assert(input.action_released('confirm') == false)\n\
                 assert(input.pad_button(0) == true)\n\
                 assert(input.pad_button(1) == false)\n\
                 assert(input.axis('horizontal') == 0, 'physical axis unaffected by gamepad')\n\
                 assert(input.action_axis('move_left', 'move_right') == 0)\n\
                 return tostring(input.action('confirm'))",
            )
            .unwrap();
        assert_eq!(raw, "true");
        // 手柄轴：动作过 0.5 阈值，模拟量按方向符号取正。
        let mut input = InputSnapshot::default();
        input.set_gamepad_axis(1, 0.8);
        runtime.set_input(input);
        runtime
            .execute_console(
                "assert(input.action('ui_up') == true, 'stick up drives ui_up')\n\
                 assert(input.action('ui_down') == false)\n\
                 assert(input.action('confirm') == false)\n\
                 assert(math.abs(input.pad_axis(1) - 0.8) < 1e-6)\n\
                 assert(math.abs(input.action_axis('ui_down', 'ui_up') - 0.8) < 1e-6, 'up is positive')\n\
                 assert(math.abs(input.action_axis('stick_up', 'stick_down')) < 1e-6)\n\
                 return 'ok'",
            )
            .unwrap();
        // 自定义摇杆动作（负向在前、正向在后）→ 模拟量而非 0/1。
        runtime
            .execute_console(
                "assert(select(1, input.rebind('stick_down', 'padaxis:1-')) == true)\n\
                 assert(select(1, input.rebind('stick_up', 'padaxis:1+')) == true)\n\
                 assert(math.abs(input.action_axis('stick_down', 'stick_up') - 0.8) < 1e-6)\n\
                 return 'ok'",
            )
            .unwrap();
        // 键位绑定：轴是 0/1（而非手柄模拟量）。
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyD, true);
        runtime.set_input(input);
        runtime
            .execute_console(
                "assert(input.action('move_right') == true)\n\
                 assert(input.action_axis('move_left', 'move_right') == 1)\n\
                 return 'ok'",
            )
            .unwrap();
        // 边沿在本逻辑帧内保留，`finish_frame` 后清除（由宿主每帧调用）。
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyE, true);
        input.finish_frame();
        runtime.set_input(input);
        runtime
            .execute_console(
                "assert(input.action('interact') == true)\n\
                 assert(input.action_pressed('interact') == false)\n\
                 return 'ok'",
            )
            .unwrap();
    }

    /// M20-07：重绑定 / 解绑 / 重置 / 映射文本往返，含明确错误原因。
    #[test]
    fn input_action_map_rebind_unbind_and_text_round_trip() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime
            .execute_console(
                "assert(input.rebind('interact', 'key:f') == true)\n\
                 local list = input.bindings('interact')\n\
                 assert(#list == 1 and list[1] == 'key:keyf', 'v1 写法键位读回必须是 v2 物理键名')\n\
                 assert(input.rebind('interact', 'key:keye') == true)\n\
                 assert(input.bindings('interact')[1] == 'key:keye', 'v2 物理键名也可直接重绑定')\n\
                 assert(input.rebind('interact', 'key:f') == true)\n\
                 return 'ok'",
            )
            .unwrap();
        // 旧绑定失效、新绑定生效。
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyE, true);
        runtime.set_input(input);
        runtime
            .execute_console("assert(input.action('interact') == false); return 'ok'")
            .unwrap();
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyF, true);
        runtime.set_input(input);
        runtime
            .execute_console(
                "assert(input.action('interact') == true)\n\
                 assert(input.action_pressed('interact') == true)\n\
                 return 'ok'",
            )
            .unwrap();
        // 物理键 API 不受重绑定影响（证明只加了动作层）。M22/W1：键名同时接受
        // v1 可读名（`f`/`e`）与 v2 物理键名（`keyf`/`keye`），解析到同一物理键。
        runtime
            .execute_console(
                "assert(input.is_down('f') == true)\n\
                 assert(input.is_down('e') == false)\n\
                 assert(input.is_down('keyf') == true, 'v2 物理键名与 v1 可读名同源')\n\
                 assert(input.pressed('f') == true)\n\
                 return 'ok'",
            )
            .unwrap();
        // 未知名报错（旧实现静默落 Other(0)，拼错与真按键无法区分）。
        let error = runtime
            .execute_console("return input.is_down('not-a-key')")
            .expect_err("未知名必须报错");
        assert!(
            format!("{error:?}").contains("not-a-key"),
            "错误信息要带上原值：{error:?}"
        );
        // 非法绑定 / 重复绑定 → false + 明确原因，不 panic，也不破坏当前绑定。
        let raw = runtime
            .execute_console(
                "local ok, why = input.rebind('interact', 'joystick:1')\n\
                 assert(ok == false and why:find('joystick:1') ~= nil, tostring(why))\n\
                 assert(input.bindings('interact')[1] == 'key:keyf', 'invalid rebind must not clear bindings')\n\
                 local dup_ok, dup_why = input.rebind('interact', 'key:f')\n\
                 assert(dup_ok == true, 'rebind is an overwrite, not a duplicate add')\n\
                 local bad = input.rebind('interact', 'not-a-binding')\n\
                 assert(bad == false)\n\
                 return tostring(ok) .. '|' .. tostring(dup_ok) .. '|' .. tostring(bad)",
            )
            .unwrap();
        assert_eq!(raw, "false|true|false");
        // 解绑 → 动作不再命中；重置 → 恢复默认绑定。
        runtime
            .execute_console(
                "assert(input.unbind('interact', 'key:f') == true)\n\
                 assert(input.unbind('interact', 'key:f') == false)\n\
                 assert(#input.bindings('interact') == 0)\n\
                 assert(input.unbind('interact', 'key:e') == false)\n\
                 input.reset_bindings()\n\
                 local list = input.bindings('interact')\n\
                 assert(#list == 1 and list[1] == 'key:keye')\n\
                 return 'ok'",
            )
            .unwrap();
        // 映射文本往返：导出 → 改绑 → 导入 → 与导出时一致；版本头不符 → false + 原因。
        let raw = runtime
            .execute_console(
                "local text = input.action_map_text()\n\
                 input.rebind('interact', 'key:g')\n\
                 assert(input.bindings('interact')[1] == 'key:keyg')\n\
                 assert(select(1, input.set_action_map(text)) == true)\n\
                 assert(input.bindings('interact')[1] == 'key:keye')\n\
                 local ok, why = input.set_action_map('rustlua-actions-v0\\ninteract=key:e')\n\
                 assert(ok == false and why:find('rustlua-actions-v0', 1, true) ~= nil, tostring(why))\n\
                 assert(input.bindings('interact')[1] == 'key:keye', 'bad version must not apply')\n\
                 return tostring(ok)",
            )
            .unwrap();
        assert_eq!(raw, "false");
        // 宿主注入的映射对脚本立即可见（单一事实来源）。
        let mut map = ActionMap::with_defaults();
        map.clear("interact");
        map.bind(
            "interact",
            Binding::Key(KeyCode::from_char('z').expect("QWERTY 字符必须有对应物理键")),
        )
        .unwrap();
        runtime.set_action_map(map);
        runtime
            .execute_console(
                "assert(input.bindings('interact')[1] == 'key:keyz')\n\
                 assert(input.is_down('z') == false)\n\
                 return 'ok'",
            )
            .unwrap();
    }

    /// M20-07：`[input]` 段设置文件往返 —— 与内存映射相等、保留 `[audio]` 段与
    /// 注释、版本头不符回退默认并产生可定位诊断、无工程根返回明确错误。
    #[test]
    fn input_action_settings_round_trip_and_version_fallback() {
        let root = std::env::temp_dir().join(format!(
            "rustlua-lua-input-settings-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.toml");
        fs::write(
            &settings_path,
            "# 玩家设置\n[audio]\nmaster = 0.5\nbgm = 0.25\nsfx = 1.0\nui = 1.0\n",
        )
        .unwrap();
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        let path = runtime
            .execute_console("assert(input.settings_path() ~= nil); return input.settings_path()")
            .unwrap();
        assert!(path.ends_with("settings.toml"), "{path}");
        // 改绑 → 写盘 → 读盘：设置文件里的映射与内存映射相等。
        runtime
            .execute_console(
                "assert(input.rebind('interact', 'key:f') == true)\n\
                 assert(select(1, input.save_settings()) == true)\n\
                 input.reset_bindings()\n\
                 assert(input.bindings('interact')[1] == 'key:keye')\n\
                 assert(select(1, input.load_settings()) == true)\n\
                 assert(input.bindings('interact')[1] == 'key:keyf', 'saved binding must survive round trip')\n\
                 return 'ok'",
            )
            .unwrap();
        let text = fs::read_to_string(&settings_path).unwrap();
        // 写入 [input] 后 [audio] 段与注释仍在（保序读改写）。
        assert!(text.contains("# 玩家设置"), "{text}");
        assert!(text.contains("master = 0.5"), "{text}");
        assert!(text.contains("bgm = 0.25"), "{text}");
        assert!(text.contains("[input]"), "{text}");
        assert!(text.contains("[audio]"), "{text}");
        // M22/W1：写盘的是 v2 物理键文本。
        assert!(text.contains("rustlua-actions-v2"), "{text}");
        assert!(text.contains("interact=key:keyf"), "{text}");
        // 设置文件文本 == 当前内存映射的 to_text（宿主与脚本同源）。
        assert_eq!(runtime.action_map().to_text(), {
            let start = text.find("map = \"").expect("map line") + "map = \"".len();
            let rest = &text[start..];
            let end = rest.find('"').expect("closing quote");
            rest[..end].replace("\\n", "\n")
        });
        assert!(runtime.input_diagnostics().is_empty());
        // 旧物理键 API 仍然可用。
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyF, true);
        runtime.set_input(input);
        runtime
            .execute_console(
                "assert(input.action('interact') == true)\n\
                 assert(input.is_down('f') == true)\n\
                 return 'ok'",
            )
            .unwrap();
        // 版本头不符 → 回退默认 + 可定位诊断，不崩溃、不静默。
        fs::write(
            &settings_path,
            "[input]\nmap = \"rustlua-actions-v0\\ninteract=key:q\"\n",
        )
        .unwrap();
        let raw = runtime
            .execute_console(
                "local ok, why = input.load_settings()\n\
                 assert(ok == false and why:find('rustlua-actions-v0', 1, true) ~= nil, tostring(why))\n\
                 assert(input.bindings('interact')[1] == 'key:keye', 'fallback to defaults')\n\
                 local list = input.diagnostics()\n\
                 assert(#list >= 1 and list[#list]:find('reverting to defaults') ~= nil, tostring(#list))\n\
                 return list[#list]",
            )
            .unwrap();
        assert!(raw.contains("settings.toml"), "{raw}");
        // 非法绑定文本 → 同样回退默认并给可定位原因。
        runtime
            .execute_console(
                "assert(input.rebind('interact', 'key:r') == true)\n\
                 return 'ok'",
            )
            .unwrap();
        fs::write(
            &settings_path,
            "[input]\nmap = \"rustlua-actions-v1\\ninteract=joystick:1\"\n",
        )
        .unwrap();
        let raw = runtime
            .execute_console(
                "local ok, why = input.load_settings()\n\
                 assert(ok == false and why:find('joystick:1') ~= nil, tostring(why))\n\
                 assert(input.bindings('interact')[1] == 'key:keye')\n\
                 return why",
            )
            .unwrap();
        assert!(raw.contains("joystick:1"), "{raw}");
        // 缺 [input] 段（或文件缺失）→ 保留当前绑定 + 诊断，不清空用户改键。
        fs::write(&settings_path, "[audio]\nmaster = 1.0\n").unwrap();
        runtime
            .execute_console(
                "assert(input.rebind('interact', 'key:v') == true)\n\
                 local ok, why = input.load_settings()\n\
                 assert(ok == false and why:find('keeping current bindings') ~= nil, tostring(why))\n\
                 assert(input.bindings('interact')[1] == 'key:keyv', 'missing [input] must not wipe bindings')\n\
                 return 'ok'",
            )
            .unwrap();
        let _ = fs::remove_file(&settings_path);
        let raw = runtime
            .execute_console(
                "local ok, why = input.load_settings()\n\
                 assert(ok == false and why == 'missing', tostring(why))\n\
                 assert(input.bindings('interact')[1] == 'key:keyv')\n\
                 return why",
            )
            .unwrap();
        assert_eq!(raw, "missing");
        // 显式绝对路径：无工程根也能读（无头/测试场景）。
        let alt = root.join("alt.toml");
        fs::write(
            &alt,
            "[input]\nmap = \"rustlua-actions-v1\\ninteract=key:m\"\n",
        )
        .unwrap();
        runtime
            .execute_console(&format!(
                "assert(select(1, input.load_settings([==[{}]==])) == true)\n\
                 assert(input.bindings('interact')[1] == 'key:keym')\n\
                 return 'ok'",
                alt.display()
            ))
            .unwrap();
        // 无工程根且未给路径 → 明确错误（不 panic）。
        let mut rootless = LuaRuntime::new().unwrap();
        assert_eq!(
            rootless
                .execute_console("return select(2, input.save_settings())")
                .unwrap(),
            "no_project_root"
        );
        assert_eq!(
            rootless
                .execute_console("return select(2, input.load_settings())")
                .unwrap(),
            "no_project_root"
        );
        // 绑定不随场景切换/读档回滚：切场景只清 ECS/UI，保留玩家改键。
        let scene_dir = root.join("scenes");
        fs::create_dir_all(&scene_dir).unwrap();
        let mut scene = crate::core::scene::Scene::new("town");
        scene
            .add_entity(crate::core::scene::SceneEntityId(1), "Player", None)
            .unwrap();
        scene.save(scene_dir.join("town.scene")).unwrap();
        runtime.set_action_map({
            let mut map = ActionMap::with_defaults();
            map.clear("interact");
            map.bind(
                "interact",
                Binding::Key(KeyCode::from_char('y').expect("QWERTY 字符必须有对应物理键")),
            )
            .unwrap();
            map
        });
        runtime
            .execute_console("assert(select(1, scene.load('town')) == true); return 'ok'")
            .unwrap();
        runtime.process_scene_transition().unwrap();
        runtime
            .execute_console(
                "assert(input.bindings('interact')[1] == 'key:keyy', 'scene switch keeps user bindings')\n\
                 return 'ok'",
            )
            .unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    /// P3：玩家设置根与工程根分离（发布包）。`settings_root` 指向用户数据目录后，
    /// `input.*` 与 `audio.*` 的 `settings_path/load_settings/save_settings` 全部落到
    /// 用户目录，**一个字节都不写发布包**；不设置时退回工程根（开发工程形态）。
    #[test]
    fn settings_root_redirects_player_settings_out_of_the_release_package() {
        let base = std::env::temp_dir().join(format!(
            "rustlua-settings-root-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pack = base.join("pack");
        let user = base.join("user");
        fs::create_dir_all(&pack).unwrap();
        fs::create_dir_all(&user).unwrap();
        // 发布包自带一份默认设置（首次运行由宿主播种到用户目录）。
        fs::write(
            pack.join("settings.toml"),
            "[audio]\nmaster = 0.5\nbgm = 0.25\nsfx = 1.0\nui = 1.0\n",
        )
        .unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(pack.clone());
        // 未设置设置根 → 仍是开发工程形态：读写工程内的 settings.toml。
        assert_eq!(
            runtime
                .execute_console("return input.settings_path()")
                .unwrap(),
            pack.join("settings.toml").to_string_lossy()
        );
        // 指向用户目录（发布包形态）。
        runtime.set_settings_root(user.clone());
        assert_eq!(
            runtime
                .execute_console("return input.settings_path()")
                .unwrap(),
            user.join("settings.toml").to_string_lossy()
        );
        assert_eq!(
            runtime
                .execute_console("return audio.settings_path()")
                .unwrap(),
            user.join("settings.toml").to_string_lossy()
        );
        // 改键 + 音量落盘 → 都写进用户目录，发布包目录保持原样。
        runtime
            .execute_console(
                "assert(input.rebind('interact', 'key:f') == true)\n\
                 assert(select(1, input.save_settings()) == true)\n\
                 assert(select(1, audio.save_settings()) == true)\n\
                 return 'ok'",
            )
            .unwrap();
        let user_text = fs::read_to_string(user.join("settings.toml")).unwrap();
        assert!(user_text.contains("interact=key:keyf"), "{user_text}");
        assert!(user_text.contains("[audio]"), "{user_text}");
        assert_eq!(
            fs::read_to_string(pack.join("settings.toml")).unwrap(),
            "[audio]\nmaster = 0.5\nbgm = 0.25\nsfx = 1.0\nui = 1.0\n",
            "发布包不得被写入玩家设置"
        );
        // 相对路径参数也相对设置根解析（不是工程根）。
        runtime
            .execute_console(
                "assert(select(1, input.save_settings('player/custom.toml')) == true)\n\
                 return 'ok'",
            )
            .unwrap();
        assert!(user.join("player").join("custom.toml").is_file());
        assert!(!pack.join("player").exists(), "不得在发布包里建目录");
        let _ = fs::remove_dir_all(&base);
    }

    /// M20-07：动作层可驱动面板导航（`ui.focus_*`）——默认绑定（方向键 + 左摇杆）
    /// 直接可用，面板不需要自己解析物理键。
    #[test]
    fn input_actions_drive_ui_focus_navigation() {
        let mut runtime = LuaRuntime::new().unwrap();
        // 左摇杆上推 → `ui_up` 命中，合成轴为 +0.9。
        let mut input = InputSnapshot::default();
        input.set_gamepad_axis(1, 0.9);
        runtime.set_input(input);
        runtime
            .execute_console(
                "assert(input.action('ui_up') == true)\n\
                 assert(input.action('ui_down') == false)\n\
                 assert(math.abs(input.action_axis('ui_down', 'ui_up') - 0.9) < 1e-6)\n\
                 return 'ok'",
            )
            .unwrap();
        // 面板把（过阈值的）动作 + 模拟量接到 M20-02 的焦点 API。
        runtime
            .execute_console(
                "ui.focus_open('bag', 3)\n\
                 local step = 0\n\
                 if math.abs(input.action_axis('ui_down', 'ui_up')) >= 0.5 then\n\
                   step = input.action_axis('ui_down', 'ui_up') > 0 and 1 or -1\n\
                 end\n\
                 assert(step == 1)\n\
                 assert(ui.focus_move('bag', 'down') == 0)\n\
                 assert(ui.focus_move('bag', 'down') == 1)\n\
                 assert(ui.focus_index('bag') == 1)\n\
                 return 'ok'",
            )
            .unwrap();
        // 手柄 A/B → `confirm` / `cancel`：焦点环上确认与取消都成立。
        let mut input = InputSnapshot::default();
        input.gamepad_button_event(0, true);
        runtime.set_input(input);
        runtime
            .execute_console(
                "assert(input.action('confirm') == true)\n\
                 local confirmed = false\n\
                 if input.action_pressed('confirm') then confirmed = ui.focus_select('bag', 2) end\n\
                 assert(confirmed == true and ui.focus_index('bag') == 2)\n\
                 return 'ok'",
            )
            .unwrap();
        let mut input = InputSnapshot::default();
        input.gamepad_button_event(1, true);
        runtime.set_input(input);
        runtime
            .execute_console(
                "assert(input.action('cancel') == true)\n\
                 local closed = false\n\
                 if input.action('cancel') then closed = ui.focus_close('bag') end\n\
                 assert(closed == true and ui.focus_count('bag') == 0)\n\
                 return 'ok'",
            )
            .unwrap();
        // 方向键走同一动作层：↓ 键 → `ui_down` → 焦点下移（键盘与手柄同源）。
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::ArrowDown, true);
        runtime.set_input(input);
        runtime
            .execute_console(
                "assert(input.is_down('down') == true, 'physical arrow key works')\n\
                 assert(input.action('ui_down') == true, 'arrows drive the same actions as the stick')\n\
                 ui.focus_open('arrows', 3)\n\
                 local first, second = nil, nil\n\
                 if input.action('ui_down') then first = ui.focus_move('arrows', 'down') end\n\
                 if input.action('ui_down') then second = ui.focus_move('arrows', 'down') end\n\
                 assert(first == 0 and second == 1, 'arrow key drives focus through the action layer')\n\
                 assert(ui.focus_index('arrows') == 1)\n\
                 return 'ok'",
            )
            .unwrap();
    }

    /// M20-02：`ui.focus_*` 焦点环——方向移动、环绕、网格列、越界与未知方向。
    #[test]
    fn ui_focus_lua_api_navigates_lists_and_grids() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime
            .execute_console(
                // 单列 3 项：下/上移动 + 边界环绕。
                "assert(ui.focus_open('list', 3) == 3)\n\
                 assert(ui.focus_count('list') == 3)\n\
                 assert(ui.focus_index('list') == nil)\n\
                 assert(ui.focus_move('list', 'down') == 0)\n\
                 assert(ui.focus_move('list', 'down') == 1)\n\
                 assert(ui.focus_move('list', 'down') == 2)\n\
                 assert(ui.focus_move('list', 'down') == 0)\n\
                 assert(ui.focus_move('list', 'up') == 2)\n\
                 assert(ui.focus_select('list', 1) == true)\n\
                 assert(ui.focus_index('list') == 1)\n\
                 assert(ui.focus_select('list', 9) == false)\n\
                 assert(ui.focus_cycle('list', true) == 2)\n\
                 assert(ui.focus_cycle('list', false) == 1)\n\
                 do local ok = pcall(function() return ui.focus_move('list', 'sideways') end)\n\
                    assert(not ok, 'unknown direction must error') end\n\
                 assert(ui.focus_move('missing', 'down') == nil)\n\
                 assert(ui.focus_close('list') == true)\n\
                 assert(ui.focus_count('list') == 0)\n\
                 return 'ok'",
            )
            .unwrap();
        runtime
            .execute_console(
                // 2 列网格：下移同列、右移同行、边界环绕。
                "ui.focus_open('grid', 4, 2)\n\
                 assert(ui.focus_select('grid', 0) == true)\n\
                 assert(ui.focus_move('grid', 'down') == 2)\n\
                 assert(ui.focus_move('grid', 'down') == 0)\n\
                 assert(ui.focus_move('grid', 'right') == 1)\n\
                 assert(ui.focus_move('grid', 'right') == 0)\n\
                 assert(ui.focus_cycle('grid', true) == 1)\n\
                 return 'ok'",
            )
            .unwrap();
    }

    /// M20-02：`ui.scroll_*` 滚动状态——钳制、按行、滚入视口与可见区间。
    #[test]
    fn ui_scroll_lua_api_clamps_and_reveals_items() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime
            .execute_console(
                "ui.scroll_open('s', 100, 250)\n\
                 assert(math.abs(ui.scroll_max('s') - 150) < 1e-6)\n\
                 assert(math.abs(ui.scroll_by('s', 30) - 30) < 1e-6)\n\
                 assert(math.abs(ui.scroll_offset('s') - 30) < 1e-6)\n\
                 assert(math.abs(ui.scroll_by('s', -100) + 30) < 1e-6)\n\
                 assert(math.abs(ui.scroll_lines('s', 2, 20) - 40) < 1e-6)\n\
                 assert(math.abs(ui.scroll_page('s', 1) - 100) < 1e-6)\n\
                 assert(math.abs(ui.scroll_offset('s') - 140) < 1e-6)\n\
                 assert(math.abs(ui.scroll_page('s', 1) - 10) < 1e-6)\n\
                 assert(math.abs(ui.scroll_offset('s') - 150) < 1e-6)\n\
                 assert(math.abs(ui.scroll_page('s', 1)) < 1e-6)\n\
                 assert(ui.scroll_to_visible('s', 20, 30) == true)\n\
                 assert(math.abs(ui.scroll_offset('s') - 20) < 1e-6)\n\
                 assert(ui.scroll_to_visible('s', 20, 30) == false)\n\
                 local range = ui.scroll_visible('s')\n\
                 assert(math.abs(range.start - 20) < 1e-6 and math.abs(range.stop - 120) < 1e-6)\n\
                 ui.scroll_extents('s', 100, 60)\n\
                 assert(math.abs(ui.scroll_max('s')) < 1e-6)\n\
                 assert(math.abs(ui.scroll_offset('s')) < 1e-6)\n\
                 assert(ui.scroll_offset('missing') == nil)\n\
                 assert(ui.scroll_close('s') == true)\n\
                 return 'ok'",
            )
            .unwrap();
    }

    /// M20-02 + M22/W1：方向键是物理键（宿主喂入），`parse_key_strict` 同时认
    /// v1 可读名（`up`/`down`/`left`/`right`）与 v2 物理键名（`arrowup`…）。
    #[test]
    fn arrow_keys_are_physical_keys_distinct_from_letters() {
        assert_eq!(parse_key_strict("up").unwrap(), KeyCode::ArrowUp);
        assert_eq!(parse_key_strict("DOWN").unwrap(), KeyCode::ArrowDown);
        assert_eq!(parse_key_strict("Left").unwrap(), KeyCode::ArrowLeft);
        assert_eq!(parse_key_strict("right").unwrap(), KeyCode::ArrowRight);
        // v2 物理键名与 v1 可读名必须解析到同一物理键。
        assert_eq!(parse_key_strict("arrowup").unwrap(), KeyCode::ArrowUp);
        assert_eq!(parse_key_strict("key:arrowup").unwrap(), KeyCode::ArrowUp);
        assert_ne!(
            parse_key_strict("up").unwrap(),
            parse_key_strict("w").unwrap()
        );
        // 未知名报错（旧实现静默落 Other(0)，拼错与真按键无法区分）。
        let error = parse_key_strict("not-a-key").unwrap_err().to_string();
        assert!(error.contains("not-a-key"), "{error}");
        assert!(parse_key_strict("").is_err());
        let mut runtime = LuaRuntime::new().unwrap();
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::ArrowDown, true);
        runtime.set_input(input);
        assert_eq!(
            runtime
                .execute_console(
                    "return tostring(input.just_pressed('down')) .. '|' .. tostring(input.is_down('w'))"
                )
                .unwrap(),
            "true|false"
        );
    }
}
