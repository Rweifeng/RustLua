use mlua::{Lua, Result as LuaResult, Value};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
};

// WP11 阶段 1：`script.rs` 拆为子目录。这里保留的骨架是**共享状态 + 运行时**，
// 搬出去的是三条自持的下层：错误/诊断、值桥、源码读取与 require 装配。
//
// `ScriptState` 刻意留在本文件（私有域 = `script` 模块）：Rust 的私有项对**定义模块及其所有
// 后代模块**可见，因此 `script::api::*`、`script::values` 都能直接读写它这 40 个私有字段，
// 不需要为拆分而把它们放宽成 `pub(super)`。反过来若把 `ScriptState` 移进 `state.rs`，
// 兄弟子模块就访问不到，得逐字段加属性——所以宁可让这个结构体待在最显眼的地方。
mod api;
mod error;
mod guard;
mod require;
mod runtime;
mod scene_spawn;
mod step;
mod transition;
mod values;

// 跨 crate 的公开面（`editor` / `game_web` / `host` 用到）：消费方沿用的 `engine::core::script::*`
// 由 engine 的 core.rs 门面再导出，路径与拆分前一字不变。
pub use error::{LuaBridgeError, ScriptDebugger, ScriptDiagnostic};
pub use guard::{
    BudgetSlot, CallbackBudget, ContainOutcome, DEFAULT_CALLBACK_ERROR_LIMIT, EntityHook,
    ScriptErrorPolicy, ScriptGuard,
};
pub use require::{install_web_require, read_project_script};
pub use values::entity_lua_id;

// WP11-USE:BEGIN
use error::{ScriptFingerprint, api_error};
use rustlua_animation::animation::AnimationFrameEvent;
use rustlua_assets::resource_db::AssetId;
use rustlua_audio::audio::{AudioBus, AudioSystem};
use rustlua_camera::camera::{Camera2D, Camera3D, Viewport};
use rustlua_ecs::{
    entity::EntityId,
    particle::{Particle, ParticleEmitter, ParticleSystem},
    tilemap::Tilemap,
    transform::Transform,
    world::World,
};
use rustlua_gameplay::{
    gameplay::{GameClock, Inventory, ItemDefinition, ItemStack},
    settings::{NavSettings, ProjectSettings},
};
use rustlua_input::input::{ActionMap, InputSnapshot, KeyCode, MouseButton};
use rustlua_math::math2d::Vec2;
#[allow(unused_imports)] // 经 `script::tests` 的 `use super::*` 链供 `tests/camera.rs` 使用
use rustlua_math::math3d::Vec3;
use rustlua_physics::{
    physics::{ColliderDesc, ColliderShape, PhysicsWorld},
    physics3d::PhysicsWorld3d,
};
use rustlua_scene::scene::SceneEntityId;
use rustlua_ui::{
    ui::{DynamicUiImage, UiGrid, UiPanel},
    ui_navigation::{FocusRing, ScrollState},
};
use values::{format_lua_value, lua_value_to_json};
// WP11-USE:END

/// 运行期 API 面的**事实源探针**：遍历 `_G`，剔除 Lua 标准库，导出「模块表 → 函数名（排序）」。
///
/// 两条门共用它，因此不会各写一份枚举而漂移：
/// - `host::sample_project` 的 API 覆盖率门（新增函数必须有演示）；
/// - `lua_api_surface_matches_baseline` 的快照门（清单必须与提交进仓库的基线一致）。
///
/// 快照门是 WP11（`tasks/done/WP11-script-rs拆分.md`）拆分 `script.rs` 的行为不变量守卫：
/// `register_api` 有过「同名函数注册两次、后者静默覆盖」的事故，人眼 diff 抓不住。
pub const LUA_API_PROBE: &str = r#"
(function()
    local stdlib = {
        string=true, table=true, math=true, io=true, os=true, coroutine=true,
        debug=true, utf8=true, package=true, _G=true, bit32=true, arg=true,
    }
    local out = {}
    for key, value in pairs(_G) do
        if type(value) == "table" and not stdlib[key] then
            local funcs = {}
            for field, inner in pairs(value) do
                if type(inner) == "function" then
                    funcs[#funcs + 1] = tostring(field)
                end
            end
            table.sort(funcs)
            out[#out + 1] = { name = tostring(key), funcs = funcs }
        end
    end
    table.sort(out, function(a, b) return a.name < b.name end)
    return { tables = out }
end)()
"#;

/// docs/60 §4.1：`ScriptState` 按领域拆分后的**实体注册域**。
///
/// 场景切换清理走 [`EntityState::clear_for_scene_transition`]——新字段若属于场景
/// 生命周期，加进这里而不是散落的 `state.x.clear()` 行。
#[derive(Debug, Default)]
struct EntityState {
    names: BTreeMap<String, EntityId>,
    health: HashMap<EntityId, f32>,
    /// Stable project-level entity IDs (`persist_id` → live entity). Distinct
    /// from `names` (display/authoring name, collision-prone): a persist ID is
    /// unique across the runtime and is what save/snapshot uses to re-bind
    /// runtime entities (crops, buildings, containers, NPCs) without
    /// duplicating or misbinding them.
    persist: HashMap<String, EntityId>,
    /// Persistent entities (name → authored snapshot) that survive scene loads.
    persistent: BTreeMap<String, PersistedEntity>,
    /// Per-entity string tags used by interact queries and gameplay scripts.
    /// P3-①：纯按键查找、从不按序遍历（有序性只在存档/序列化侧需要），故用 HashMap。
    tags: HashMap<EntityId, HashMap<String, String>>,
    /// Optional on-screen interaction prompt text (`interact.prompt`).
    interact_prompt: Option<String>,
    /// Authoring ID → live ECS handle. The editor uses this to preview a drag
    /// by mutating one component without rebuilding the whole runtime.
    scene_entities: BTreeMap<SceneEntityId, EntityId>,
}

impl EntityState {
    /// 场景切换时丢弃**当前场景**的注册数据；`persistent` 刻意保留（跨场景快照）。
    fn clear_for_scene_transition(&mut self) {
        self.names.clear();
        self.persist.clear();
        self.health.clear();
        self.tags.clear();
        self.scene_entities.clear();
        self.interact_prompt = None;
    }
}

/// docs/60 §4.1：动态 UI 域（文本/条/面板/图/网格/拖拽/焦点/滚动 + 视口参考系）。
#[derive(Debug, Default)]
struct UiState {
    texts: BTreeMap<String, UiText>,
    bars: BTreeMap<String, UiBar>,
    panels: BTreeMap<String, UiPanel>,
    images: BTreeMap<String, DynamicUiImage>,
    grids: BTreeMap<String, UiGrid>,
    /// `ui.scene_text_visible(name, false)` 登记的场景文字实体按名隐藏集。
    /// 语义与 web 兼容层 `__rustlua_scene_text_hidden` 对齐：实体仍在场景里
    /// （照常参与 UiLayout 布局），仅绘制场景文字时跳过该实体。
    scene_text_hidden: BTreeSet<String>,
    drag: Option<UiDrag>,
    /// 运行时 UI 焦点环（M20-02）：`ui.focus_open/move/...` 按稳定 id 持有，
    /// 面板用方向键/摇杆方向驱动选择（`rustlua_ui::ui_navigation::FocusRing`）。
    focus_rings: HashMap<String, FocusRing>,
    /// 运行时列表滚动状态（M20-02）：面板把选中行滚入视口
    /// （`rustlua_ui::ui_navigation::ScrollState`）。
    scroll_states: HashMap<String, ScrollState>,
    viewport: [f32; 2],
    /// Canvas 空间 UI 参考分辨率（设计分辨率），与运行时渲染保持一致，
    /// 供 Lua 侧 `process_ui_clicks` / `grid_hit` 命中测试使用。
    reference: [f32; 2],
}

impl UiState {
    /// 场景切换时丢弃动态登记的 UI 元素与交互状态；`viewport`/`reference`（窗口
    /// 形态）与 `scene_text_hidden`（沿用既有行为，按名隐藏跨场景仍生效）保留。
    fn clear_for_scene_transition(&mut self) {
        self.texts.clear();
        self.bars.clear();
        self.panels.clear();
        self.images.clear();
        self.grids.clear();
        self.drag = None;
        self.focus_rings.clear();
        self.scroll_states.clear();
    }
}

/// docs/60 §4.1：物理模拟域（2D/3D 世界 + 分阶段回写 + 调试绘制请求）。
#[derive(Debug, Default)]
struct PhysicsState {
    world2d: PhysicsWorld,
    world3d: PhysicsWorld3d,
    /// WP7：`Physics` 阶段推进后被移动的动态刚体，等 `Writeback` 阶段再写进 ECS。
    /// 拆开两个阶段是为了让 2D/3D 的**所有**推进都发生在**任何**回写之前
    /// （此前 2D 的回写夹在 3D 推进之前，"谁先看到谁"取决于注册顺序）。
    pending_writeback_2d: Vec<(EntityId, Vec2)>,
    pending_writeback_3d: Vec<(EntityId, rustlua_math::math3d::Vec3)>,
    /// WP7 ②：collider 调试绘制的**请求值**。绘制开关属于宿主（渲染在那里发生），
    /// 这里只登记 Lua 的请求、由宿主每帧取走，与 `camera2d` 的请求/消费模式一致。
    collider_debug_request: Option<bool>,
    /// docs/54 §8-⑥：3D 侧的同一条请求通道，由 `physics3d.debug_draw(...)` 登记。
    /// 与 2D 分开是刻意的：混合场景里"只画 2D 碰撞体"或"只画 3D"都是常见需求。
    collider_debug_request_3d: Option<bool>,
}

impl PhysicsState {
    /// 场景切换重建两个模拟世界（旧实体句柄全部作废）。
    fn clear_for_scene_transition(&mut self) {
        self.world2d = PhysicsWorld::new();
        self.world3d = PhysicsWorld3d::new();
    }
}

/// docs/60 §4.1：物品/背包/商店域。
#[derive(Debug, Default)]
struct InventoryState {
    item_defs: HashMap<String, ItemDefinition>,
    inventories: HashMap<String, Inventory>,
    /// Shop catalogs: name -> `{ item_id: { buy, sell } }` (M16-10).
    shops: HashMap<String, serde_json::Value>,
}

impl InventoryState {
    /// 场景切换只丢弃商店目录；物品定义与背包内容随存档/持久实体存活（既有语义）。
    fn clear_for_scene_transition(&mut self) {
        self.shops.clear();
    }
}

/// docs/60 §4.1：存档域（键值数据 + 存档路径/槽位）。刻意**不**随场景切换清理。
#[derive(Debug, Default)]
struct SaveState {
    data: BTreeMap<String, String>,
    path: Option<PathBuf>,
    slot: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct ScriptState {
    delta_seconds: f32,
    quit_requested: bool,
    input: InputSnapshot,
    world: World,
    scene: Option<String>,
    scene_pending_load: Option<String>,
    project_root: Option<PathBuf>,
    /// 懒加载的字体度量探针（`ui.measure_text` 用）：只读覆盖与 advance、
    /// 不光栅化字形、不占渲染图集内存。`None` = 从未尝试加载。
    font_probe: Option<Option<rustlua_render::font::FontProbe>>,
    /// 玩家设置根目录（P3）：发布包 → 用户数据目录（`%APPDATA%\<app>`），
    /// 开发工程 → 项目根（编辑器与验收样例直接检查工程内的 `settings.toml`）。
    /// 未设置时退回 `project_root`。
    settings_root: Option<PathBuf>,
    /// 上一次 Lua `update()` 的耗时（微秒）。宿主把它累加进 `PerformanceStats::lua_ms`
    /// ——这个指标此前只有单测在写，真实运行恒为 `0.000`（P4 接线，见 `docs/37` 5-6）。
    last_update_micros: u64,
    assets: HashMap<String, String>,
    /// WP4：2D 相机控制器（跟随 / 死区 / 平滑 / 抖动）。相机数据仍在
    /// `RuntimeScene::camera`，这里只放"center 该怎么算"的控制器状态。
    camera2d: rustlua_camera::camera::Camera2dController,
    particles: ParticleSystem,
    /// WP4 ⑦：本帧已触发、等待 Lua 轮询取走的动画帧事件（`animation.events()`）。
    animation_events: Vec<(EntityId, AnimationFrameEvent)>,
    /// WP4 ⑧：宿主回填的 2D 相机快照（相机 + 视口），供 Lua 做屏幕↔世界换算。
    camera_view: Option<(Camera2D, [f32; 2])>,
    /// docs/54 §8-⑦：宿主回填的 **3D 相机快照**（本帧 3D pass 实际用的那台 + 视口），
    /// 供 Lua 做屏幕→射线、世界→屏幕换算（`camera3d.*`）。与编辑器 picking 共用
    /// `Camera3D::ray_from_screen` / `world_to_ndc`，因此两侧不会各写一套投影数学。
    camera3d_view: Option<(Camera3D, [f32; 2])>,
    audio: AudioSystem,
    /// Entity whose `Camera` component is the **active camera**（相机拆分批 9：只有这一条选择链）。
    /// Lua selects it with `camera.set_active`; each pass then filters candidates by projection —
    /// 3D pass 取透视、2D 相机取正交——未选择（或选中的不是本 pass 的投影）时回退到排序序第一台。
    active_camera: Option<EntityId>,
    /// Entity whose `Light` component the renderer uses this frame.
    active_light: Option<EntityId>,
    game_clock: GameClock,
    data_cache: HashMap<String, serde_json::Value>,
    /// Registered `data.schema(path, spec)` validators for data tables.
    schema_defs: HashMap<String, serde_json::Value>,
    /// 注册 `map.schema(name, spec)` 的分地图持久状态校验器（B-3）。
    /// 按地图名索引（而不是按场景索引），并刻意跨过
    /// `process_scene_transition` 保留；`schema_defs` 则只跟踪数据文件校验器。
    map_state_schemas: HashMap<String, serde_json::Value>,
    /// 运行期 `prefab.spawn` 的活实例（handle → 活世界落点）。despawn 与按名查节点都走
    /// 这张表；刻意不进场景簿记——切图不该把动态 UI 实例当场景实体重生（见
    /// `scene_spawn::PrefabSpawn` 头注释）。
    prefab_spawns: HashMap<i64, scene_spawn::PrefabSpawn>,
    next_prefab_handle: i64,
    /// 运行期 spawn 出去的 UI 预设实体的**布局节点**（键 = 活实体槽位号）。
    /// 宿主每帧解算场景 UI 矩形时把这张表并进自己的快照，于是"画得出"与"点得中"
    /// 对 spawn 的实体同样成立；场景加载那一份走同一个 builder（见
    /// `rustlua_ui::ui_scene_layout::build_ui_scene_nodes`）。
    prefab_ui_nodes: BTreeMap<u64, rustlua_ui::ui_scene_layout::UiSceneNode>,
    /// docs/54 §8-②：最近一次场景加载里**坏的 `parent` 边**（悬空父/自环）的可读描述。
    /// 运行时会把这类边当成"没有父"（容错），因此必须同时可发现，否则子树会静默少一层位移。
    scene_parent_issues: Vec<String>,
    /// docs/54 §4.3：一个实体挂了多种可绘制组件的点名（渲染只按优先级画一种，但没有诊断的话
    /// "为什么另一个没画出来"无从解释）。
    scene_renderable_conflicts: Vec<String>,
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
    action_repeat: HashMap<String, ActionRepeatState>,
    /// 待处理的震动请求（M22/W2）：Lua `input.rumble(low, high, ms)` 写入，宿主在帧末取走
    /// 并交给设备层（`GamepadPoller::rumble`）。同一逻辑帧内多次调用以最后一次为准。
    rumble_request: Option<(f32, f32, u32)>,
    // ---- docs/60 §4.1：按领域拆分的子状态 ----
    entity_state: EntityState,
    ui_state: UiState,
    physics: PhysicsState,
    inventory: InventoryState,
    save: SaveState,
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
    fn font_probe(&mut self) -> Result<Option<rustlua_render::font::FontProbe>, LuaBridgeError> {
        if let Some(cached) = &self.font_probe {
            return Ok(cached.clone());
        }
        let root = self
            .project_root
            .clone()
            .ok_or_else(|| LuaBridgeError::Api("no project root configured".to_owned()))?;
        let probe = rustlua_render::font::project_font::load_probe(&root).ok();
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
    /// 换算见 [`rustlua_ui::ui::ui_text_effective_scale`]。
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

/// Result of checking a loaded script for source changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReloadStatus {
    Unchanged,
    Reloaded,
}

/// Main-thread Lua runtime for project scripts.
pub struct LuaRuntime {
    lua: Lua,
    state: Rc<RefCell<ScriptState>>,
    current_path: Option<PathBuf>,
    fingerprint: Option<ScriptFingerprint>,
    debugger: RefCell<ScriptDebugger>,
    /// 脚本错误的处置策略与各回调预算（见 [`guard`]）。
    ///
    /// 装在 `RefCell` 里是因为回调入口（`call_update` / `call_entity_scripts` /
    /// `process_ui_clicks`）取的都是 `&self`——约 200 处调用点不值得为记账改成 `&mut self`。
    /// **借用绝不跨越 Lua 调用**：一律"先调用、返回后再记账"。
    guard: RefCell<ScriptGuard>,
    /// docs/54 §4.1：**每实体脚本**。一个脚本**文件**一份 Lua 状态（实体是实例，脚本用收到的
    /// `id` 区分自己），按文件路径缓存——上万实体共享同一个文件时不会变成上万份 Lua 状态。
    entity_scripts: HashMap<String, EntityScriptState>,
    /// 已经调用过 `on_spawn(id)` 的实体：每个实体只生成一次，重载脚本不会重复触发生成。
    entity_scripts_spawned: HashSet<EntityId>,
    /// P0-②：排序后的每实体脚本请求缓存 + 建立时的 `World::revision`。
    /// 组件结构没变化时直接复用，消掉每帧"全实体遍历 + 排序 + 字符串克隆"。
    entity_script_requests: Vec<(EntityId, String)>,
    entity_script_requests_revision: u64,
}

/// 一份每实体脚本的运行时状态：Lua 状态 + 用于热重载比对的指纹。
struct EntityScriptState {
    lua: Lua,
    fingerprint: ScriptFingerprint,
}

/// 临时探针：定位 wasm 启动路径的时间 panic（验证后移除）。
#[doc(hidden)]
pub fn probe_startup_stage(stage: u8) -> Result<(), String> {
    match stage {
        0 => {
            let _ = Lua::new();
        }
        1 => {
            let _ = ScriptState::default();
        }
        2 => {
            let _ = ActionMap::with_defaults();
        }
        _ => return Err("unknown stage".to_owned()),
    }
    Ok(())
}

impl LuaRuntime {
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
        // 触屏虚拟键（`input.virtual_key`）是**跨帧保持**的按住态，脚本只在抬起时
        // 注入 `false`；宿主每帧喂入的新快照若整份替换，会把上一帧的注入清空，
        // 摇杆/虚拟按钮就永远读不到按住。这里把旧虚拟键并入新快照。
        let mut input = input;
        input.inherit_virtual_keys(&state.input);
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

    /// 读 2D 相机控制器（WP4）。相机数据本身仍在宿主侧，这里只放"center 该怎么算"。
    pub fn camera2d_controller<R>(
        &self,
        visit: impl FnOnce(&rustlua_camera::camera::Camera2dController) -> R,
    ) -> R {
        visit(&self.state.borrow().camera2d)
    }
    /// 改 2D 相机控制器（Lua `camera2d.*` 与宿主每帧推进共用）。
    pub fn camera2d_controller_mut<R>(
        &mut self,
        visit: impl FnOnce(&mut rustlua_camera::camera::Camera2dController) -> R,
    ) -> R {
        visit(&mut self.state.borrow_mut().camera2d)
    }
    /// Stores the current viewport so Lua `ui.grid_hit` can resolve screen
    /// coordinates. The host calls this before rendering each frame.
    pub fn set_ui_viewport(&mut self, width: u32, height: u32) {
        self.state.borrow_mut().ui_state.viewport = [width.max(1) as f32, height.max(1) as f32];
    }

    /// Stores the Canvas 空间 UI 参考分辨率（设计分辨率），供 Lua 侧
    /// `process_ui_clicks` / `ui.grid_hit` 命中测试与渲染保持一致。
    pub fn set_ui_reference(&mut self, reference: [f32; 2]) {
        self.state.borrow_mut().ui_state.reference = reference;
    }

    /// Sets the directory audio clips are loaded from.
    pub fn set_audio_asset_dir(&mut self, dir: PathBuf) {
        self.state.borrow_mut().audio.set_asset_dir(dir);
    }

    /// 暂停 BGM（编辑器暂停时调用）。
    pub fn pause_music(&self) {
        self.state.borrow().audio.pause_music();
    }

    /// 恢复 BGM（编辑器恢复运行时调用）。
    pub fn resume_music(&self) {
        self.state.borrow().audio.resume_music();
    }

    /// BGM 曲尾续播（宿主每帧调用一次；见 `AudioSystem::maintain_music`）。
    pub fn maintain_music(&mut self) {
        self.state.borrow_mut().audio.maintain_music();
    }

    /// Sets the project root, used to resolve `scenes/<name>.scene` on load.
    pub fn set_project_root(&mut self, root: PathBuf) {
        // P1-③：一次借用完成两件事——相邻的两次 `borrow_mut()` 合并为一个借用
        // 作用域，杜绝任何"两次借用之间被重入"的窗口。
        let mut state = self.state.borrow_mut();
        state.project_root = Some(root);
        // 换工程 → 字体也可能换：丢缓存，下次 `ui.measure_text` 重新读盘。
        state.font_probe = None;
    }

    /// Sets the file path save data is committed to / loaded from.
    pub fn set_save_path(&mut self, path: PathBuf) {
        self.state.borrow_mut().save.path = Some(path);
    }

    /// 设置玩家设置根目录（`settings.toml` 的读写根）。
    /// 发布包由宿主指向用户数据目录（见 `rustlua_infra::user_dir`），开发工程不调用，
    /// 于是退回项目根——编辑器与 `jhyt_m20_*` 检查的就是工程内那份。
    pub fn set_settings_root(&mut self, root: PathBuf) {
        self.state.borrow_mut().settings_root = Some(root);
    }

    /// Gives the host a read view of the UI text and bar elements.
    pub fn with_ui<R>(
        &self,
        f: impl FnOnce(&BTreeMap<String, UiText>, &BTreeMap<String, UiBar>) -> R,
    ) -> R {
        let state = self.state.borrow();
        f(&state.ui_state.texts, &state.ui_state.bars)
    }

    /// 场景文字按名隐藏集（`ui.scene_text_visible` 写入）的只读视图。
    #[must_use]
    pub fn scene_text_hidden(&self) -> BTreeSet<String> {
        self.state.borrow().ui_state.scene_text_hidden.clone()
    }

    /// P3-②：一次借用判定"实体的名字是否被隐藏"——此前宿主每帧为每个文字
    /// `scene_text_hidden().clone()` 整份隐藏集再 `contains`。
    #[must_use]
    pub fn is_entity_scene_text_hidden(&self, id: EntityId) -> bool {
        let state = self.state.borrow();
        state
            .entity_state
            .names
            .iter()
            .any(|(name, entity)| *entity == id && state.ui_state.scene_text_hidden.contains(name))
    }

    /// 运行期 spawn 的 UI 预设实体的布局节点（键 = 活实体槽位号）。宿主每帧解算场景
    /// UI 矩形前把它并进自己的快照，spawn 出来的 UI 才既画得出也点得中。
    ///
    /// 返回**克隆**而不是引用：`RefCell<ScriptState>` 借用期间脚本不能再借用，而宿主
    /// 解算布局时还要 `with_world`（同一个 state 的另一个字段）。空表 = 本帧没有任何
    /// spawn 实例，宿主据此跳过合并、一份现有游戏都不付额外拷贝成本。
    #[must_use]
    pub fn prefab_ui_nodes(&self) -> BTreeMap<u64, rustlua_ui::ui_scene_layout::UiSceneNode> {
        self.state.borrow().prefab_ui_nodes.clone()
    }

    /// 运行期 spawn 实例的「活实体槽位号 → 实体名」，键空间与 [`Self::prefab_ui_nodes`] 一致。
    /// 宿主把解算出的矩形报回给调用方时用它换名——槽位号还原不出 `EntityId`（缺 generation），
    /// 因此这里直接给名字，而不是让宿主反查。
    #[must_use]
    pub fn prefab_spawn_names(&self) -> BTreeMap<u64, String> {
        let state = self.state.borrow();
        state
            .prefab_spawns
            .values()
            .flat_map(|spawn| {
                spawn
                    .names
                    .iter()
                    .map(|(name, handle)| (handle.index() as u64, name.clone()))
            })
            .collect()
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
        f(
            &state.ui_state.panels,
            &state.ui_state.images,
            &state.ui_state.grids,
        )
    }

    /// Gives the host a read view of the active particles.
    pub fn with_particles<R>(&self, f: impl FnOnce(&[Particle]) -> R) -> R {
        f(self.state.borrow().particles.particles())
    }
    /// 当前存活粒子数（宿主侧读取，避免为了拿长度而借用整个切片）。
    pub fn particle_count(&self) -> usize {
        self.state.borrow().particles.len()
    }
    /// WP4 ⑧：宿主每帧把**本帧实际渲染用的** 2D 相机与视口回填进来，供 Lua 做
    /// 屏幕↔世界换算。放在帧末（渲染前）回填，因此脚本看到的是玩家刚看到的那一帧。
    pub fn set_camera_view(&mut self, camera: Camera2D, viewport: [f32; 2]) {
        self.state.borrow_mut().camera_view = Some((camera, viewport));
    }

    /// docs/54 §8-⑦：宿主每帧把**3D pass 实际用的那台相机**与视口回填进来，供 Lua
    /// `camera3d.ray` / `camera3d.world_to_screen` 使用（与编辑器 picking 同一套投影函数）。
    pub fn set_camera3d_view(&mut self, camera: Camera3D, viewport: [f32; 2]) {
        self.state.borrow_mut().camera3d_view = Some((camera, viewport));
    }

    /// 宿主侧直接生成一次粒子爆发，与 Lua `particles.burst` 走同一实现。
    ///
    /// 给宿主/基准用：不必为了发粒子而绕一趟 Lua。参数非法时**不生成粒子**
    /// （调用方应先用 [`ParticleEmitter::validate`] 报错）。
    pub fn burst_particles(&mut self, position: Vec2, emitter: &ParticleEmitter) {
        self.state
            .borrow_mut()
            .particles
            .burst_with(position, emitter);
    }
    /// Gives the host a shared read view of the runtime entity world, so the
    /// renderer draws the exact entities the Lua script is mutating.
    pub fn with_world<R>(&self, f: impl FnOnce(&World) -> R) -> R {
        f(&self.state.borrow().world)
    }
    /// WP7 ②：取走 Lua 通过 `physics.debug_draw(...)` 登记的请求（取走即清空，与
    /// `camera2d` 的 pending 请求同一语义：请求只在被消费的那一帧生效）。
    pub fn take_collider_debug_request(&mut self) -> Option<bool> {
        self.state
            .borrow_mut()
            .physics
            .collider_debug_request
            .take()
    }
    /// WP4 ⑦：宿主推进动画后把本步触发的帧事件交进来，供 Lua `animation.events()` 轮询。
    pub fn push_animation_events(&mut self, events: Vec<(EntityId, AnimationFrameEvent)>) {
        self.state.borrow_mut().animation_events.extend(events);
    }
    /// 物理世界里当前所有 2D 碰撞体的姿态与形状，供宿主做 collider 调试绘制。
    pub fn collider_debug_shapes(&self) -> Vec<rustlua_physics::physics::ColliderDebugShape> {
        self.state.borrow().physics.world2d.debug_shapes()
    }
    /// WP7 ②/docs/54 §8-⑥：取走 Lua 通过 `physics3d.debug_draw(...)` 登记的 3D 请求
    /// （与 2D 同一条"取走即清空"语义）。
    pub fn take_collider_debug_request_3d(&mut self) -> Option<bool> {
        self.state
            .borrow_mut()
            .physics
            .collider_debug_request_3d
            .take()
    }
    /// 物理世界里当前所有 3D 碰撞体的姿态与形状，供宿主做 3D collider 调试绘制。
    pub fn collider_debug_shapes_3d(
        &self,
    ) -> Vec<rustlua_physics::physics3d::ColliderDebugShape3d> {
        self.state.borrow().physics.world3d.debug_shapes()
    }
    /// Gives the host a shared write view of the runtime entity world.
    pub fn with_world_mut<R>(&mut self, f: impl FnOnce(&mut World) -> R) -> R {
        f(&mut self.state.borrow_mut().world)
    }

    /// Returns the live ECS handle created from an authored scene entity.
    #[must_use]
    pub fn scene_entity_handle(&self, id: SceneEntityId) -> Option<EntityId> {
        self.state
            .borrow()
            .entity_state
            .scene_entities
            .get(&id)
            .copied()
    }

    /// docs/54 §8-②：最近一次场景加载里**坏的 `parent` 边**（悬空父/自环）的可读描述。
    ///
    /// 空 = 每条 `parent` 都指向同一文档里的实体且不是自己。非空时调用方应把它当**数据问题**
    /// 报给作者：运行时会把断链当成链根（世界 = 自己的局部），这正是"子树静默少一层位移"的来源。
    #[must_use]
    pub fn scene_parent_issues(&self) -> Vec<String> {
        self.state.borrow().scene_parent_issues.clone()
    }

    /// docs/54 §4.3：加载期点名的"多种可绘制组件"冲突（诊断/验收用）。
    #[must_use]
    pub fn scene_renderable_conflicts(&self) -> Vec<String> {
        self.state.borrow().scene_renderable_conflicts.clone()
    }

    /// Returns the authored name registered for an entity, if any. Entities
    /// spawned from a serialized scene keep their scene name; entities spawned
    /// by Lua via `entity.spawn` are registered with the name given at spawn.
    /// Entities never named (or dropped from `names` after despawn) return `None`.
    #[must_use]
    pub fn entity_name(&self, id: EntityId) -> Option<String> {
        self.state
            .borrow()
            .entity_state
            .names
            .iter()
            .find(|(_, registered)| **registered == id)
            .map(|(name, _)| name.clone())
    }

    /// Returns the entity whose `Camera` the passes should use, if Lua selected one with
    /// `camera.set_active`（3D pass 只在它是透视相机时用它，2D 只在它是正交相机时用它）。
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

    /// 当前场景名（`scene.reload()` 返回的那个值）。
    ///
    /// 宿主与测试用它断言"切场景真的发生了"：Lua 侧读的是同一份状态，因此不存在
    /// "脚本以为切了、宿主以为没切"的第二份真源。
    #[must_use]
    pub fn scene_name(&self) -> Option<String> {
        self.state.borrow().scene.clone()
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
                reason: error.to_string(),
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
                reason: error.to_string(),
            })?;
        lua_value_to_json(value).map_err(|error| LuaBridgeError::Runtime {
            path,
            reason: error.to_string(),
        })
    }

    fn record_bridge_error(&mut self, error: &LuaBridgeError) {
        let (path, phase, reason) = error::bridge_error_parts(error);
        self.debugger.borrow_mut().record(path, phase, reason);
    }

    #[must_use]
    pub fn diagnostics(&self) -> Vec<ScriptDiagnostic> {
        self.debugger.borrow().visible()
    }

    #[must_use]
    pub fn all_diagnostics(&self) -> Vec<ScriptDiagnostic> {
        self.debugger.borrow().all().to_vec()
    }

    pub fn clear_diagnostics(&mut self) {
        self.debugger.borrow_mut().clear();
    }

    pub fn set_debug_filter(&mut self, filter: Option<String>) {
        self.debugger.borrow_mut().set_filter(filter);
    }

    /// 设置脚本错误处置策略。切换即**重置**预算与计数：编辑器 Play/Stop 与热重载
    /// 都走这里，上一局被停用的回调不该让下一局哑着。
    pub fn set_error_policy(&mut self, policy: ScriptErrorPolicy) {
        *self.guard.borrow_mut() = ScriptGuard::new(policy);
    }

    #[must_use]
    pub fn error_policy(&self) -> ScriptErrorPolicy {
        self.guard.borrow().policy()
    }

    /// 覆盖连续错误上限（验收/测试用）。生产用 [`DEFAULT_CALLBACK_ERROR_LIMIT`]。
    pub fn set_callback_error_limit(&mut self, limit: u32) {
        self.guard.borrow_mut().set_error_limit(limit);
    }

    /// 自上次重置以来被容错的脚本错误总数。
    ///
    /// 宿主用它把"脚本出过错"变成可断言的信号：容错不等于静默（见 [`ScriptErrorPolicy`]）。
    #[must_use]
    pub fn script_error_total(&self) -> u64 {
        self.guard.borrow().total_errors()
    }

    /// 已因连续报错被停用的回调（可读键，按字典序）。
    #[must_use]
    pub fn disabled_callbacks(&self) -> Vec<String> {
        self.guard.borrow().disabled_keys()
    }

    #[must_use]
    pub fn quit_requested(&self) -> bool {
        self.state.borrow().quit_requested
    }
}

fn call_optional(lua: &Lua, name: &str) -> LuaResult<()> {
    let callback: Value = lua.globals().get(name)?;
    if let Value::Function(function) = callback {
        function.call::<()>(())?;
    }
    Ok(())
}

/// 每实体脚本的钩子：`on_spawn(id)` / `on_update(id, dt)`。没有定义就跳过（脚本可以只实现一个）。
///
/// 参数用的是 **Lua 侧的实体句柄**（整数），与 `transform`/`quad`/`physics` 这些表收的 id 同一套；
/// 报错时带上脚本路径，否则"哪个实体的哪个脚本炸了"只能靠猜。
fn call_entity_hook(
    lua: &Lua,
    name: &str,
    id: EntityId,
    delta_seconds: Option<f32>,
    path: &Path,
) -> Result<(), LuaBridgeError> {
    let callback: Value = lua
        .globals()
        .get(name)
        .map_err(|error| LuaBridgeError::Runtime {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    let Value::Function(function) = callback else {
        return Ok(());
    };
    let result = match delta_seconds {
        Some(delta) => function.call::<()>((entity_lua_id(id), delta)),
        None => function.call::<()>((entity_lua_id(id),)),
    };
    result.map_err(|error| LuaBridgeError::Runtime {
        path: path.to_path_buf(),
        reason: format!("{name}: {error}"),
    })
}
/// Calls a global Lua hook (`on_day_end` / `on_day_start`) if the script defines
/// one. Missing hooks are a no-op; a hook that errors propagates the error.
fn call_optional_day_hook(lua: &mlua::Lua, name: &str) -> Result<(), mlua::Error> {
    if let Ok(function) = lua.globals().get::<mlua::Function>(name) {
        function.call::<()>(())?;
    }
    Ok(())
}
/// 取宿主每帧回填的 (2D 相机, 视口)。**还没有渲染过任何一帧时报错**而不是猜一个默认值：
/// 猜出来的世界坐标会让"点击没反应/点错位置"变成一桩无头案。
fn camera_view_from(state: &ScriptState) -> LuaResult<(Camera2D, Viewport)> {
    let (camera, viewport) = state.camera_view.ok_or_else(|| {
        api_error("camera2d screen/world conversion needs one rendered frame first")
    })?;
    let viewport =
        Viewport::new(viewport[0], viewport[1]).map_err(|error| api_error(error.to_string()))?;
    Ok((camera, viewport))
}

fn camera_view(state: &Rc<RefCell<ScriptState>>) -> LuaResult<(Camera2D, Viewport)> {
    camera_view_from(&state.borrow())
}

/// 取宿主每帧回填的 (3D 相机, 视口)。还没渲染过任何一帧时报错——与 2D 同样的理由：
/// 猜一台相机做射线，会让"点不中/点错"变成无头案。
fn camera3d_view_from(state: &ScriptState) -> LuaResult<(Camera3D, Viewport)> {
    let (camera, viewport) = state.camera3d_view.ok_or_else(|| {
        api_error("camera3d ray/screen conversion needs one rendered frame first")
    })?;
    let viewport =
        Viewport::new(viewport[0], viewport[1]).map_err(|error| api_error(error.to_string()))?;
    Ok((camera, viewport))
}

fn camera3d_view(state: &Rc<RefCell<ScriptState>>) -> LuaResult<(Camera3D, Viewport)> {
    camera3d_view_from(&state.borrow())
}

fn require_positive_tile_size(tile_size: f32) -> Result<(), mlua::Error> {
    if !tile_size.is_finite() || tile_size <= 0.0 {
        return Err(api_error("tile_size must be finite and > 0"));
    }
    Ok(())
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

fn inventory_index(index: i64) -> Result<usize, mlua::Error> {
    if index <= 0 {
        Err(api_error("inventory slot index must be positive"))
    } else {
        Ok((index - 1) as usize)
    }
}

fn inventory_clone(state: &ScriptState, id: &str) -> Result<Inventory, mlua::Error> {
    state
        .inventory
        .inventories
        .get(id)
        .cloned()
        .ok_or_else(|| api_error(format!("inventory {id} not found")))
}

/// P2-①：槽位越界检查（同背包与跨背包共用同一措辞的报错）。
fn check_inventory_slot(inventory: &Inventory, index: usize, id: &str) -> Result<(), mlua::Error> {
    if index >= inventory.capacity {
        Err(api_error(format!("inventory {id} slot out of bounds")))
    } else {
        Ok(())
    }
}

/// P2-①：物品堆叠上限——未定义的物品按 99（与历史行为一致）。
fn item_stack_limit(state: &ScriptState, item_id: &str) -> u32 {
    state
        .inventory
        .item_defs
        .get(item_id)
        .map_or(99, |definition| definition.stack)
}

/// P2-①：槽位 → 槽位合并搬运的唯一口径（同背包 / 跨背包共用）。
///
/// 规则不变：同名物品向目标堆叠至上限；目标为空槽则整段放入；目标是**别的**物品则一件不动。
/// 返回（源槽新值, 目标槽新值, 实际搬动件数）。
fn slot_merge_result(
    source: &ItemStack,
    target: &ItemStack,
    max_stack: u32,
    count: Option<i64>,
) -> (ItemStack, ItemStack, u32) {
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
    if moved == 0 {
        return (source.clone(), target.clone(), 0);
    }
    let mut from_slot = source.clone();
    from_slot.count -= moved;
    if from_slot.count == 0 {
        from_slot = ItemStack::default();
    }
    let to_slot = if target.item_id == source.item_id {
        let mut merged = target.clone();
        merged.count += moved;
        merged
    } else {
        ItemStack::new(source.item_id.clone(), moved)
    };
    (from_slot, to_slot, moved)
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
        check_inventory_slot(&inventory, from_index, from_id)?;
        check_inventory_slot(&inventory, to_index, to_id)?;
        inventory.slots.swap(from_index, to_index);
        state
            .inventory
            .inventories
            .insert(from_id.to_owned(), inventory);
        return Ok(true);
    }
    let mut from = inventory_clone(state, from_id)?;
    let mut to = inventory_clone(state, to_id)?;
    check_inventory_slot(&from, from_index, from_id)?;
    check_inventory_slot(&to, to_index, to_id)?;
    let from_slot = from.slots[from_index].clone();
    let to_slot = to.slots[to_index].clone();
    from.slots[from_index] = to_slot;
    to.slots[to_index] = from_slot;
    state.inventory.inventories.insert(from_id.to_owned(), from);
    state.inventory.inventories.insert(to_id.to_owned(), to);
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
        check_inventory_slot(&inventory, from_index, from_id)?;
        let source = inventory.slots[from_index].clone();
        if source.item_id.is_empty() {
            return Ok(0);
        }
        let max_stack = item_stack_limit(state, &source.item_id);
        if let Some(to_index) = to_index {
            check_inventory_slot(&inventory, to_index, to_id)?;
            let (from_slot, to_slot, moved) =
                slot_merge_result(&source, &inventory.slots[to_index], max_stack, count);
            inventory.slots[from_index] = from_slot;
            inventory.slots[to_index] = to_slot;
            state
                .inventory
                .inventories
                .insert(from_id.to_owned(), inventory);
            return Ok(moved as i64);
        }
        // 未指定目标槽：自动落位（先按物品整体移除，再放回到放得下的槽位）。
        let requested = count.unwrap_or(i64::from(source.count)).max(1) as u32;
        let removed = inventory.remove(&source.item_id, requested);
        let leftover = inventory.add(&source.item_id, removed, max_stack);
        state
            .inventory
            .inventories
            .insert(from_id.to_owned(), inventory);
        return Ok(removed as i64 - leftover as i64);
    }
    let mut from = inventory_clone(state, from_id)?;
    let mut to = inventory_clone(state, to_id)?;
    check_inventory_slot(&from, from_index, from_id)?;
    let source = from.slots[from_index].clone();
    if source.item_id.is_empty() {
        return Ok(0);
    }
    let max_stack = item_stack_limit(state, &source.item_id);
    if let Some(to_index) = to_index {
        check_inventory_slot(&to, to_index, to_id)?;
        let (from_slot, to_slot, moved) =
            slot_merge_result(&source, &to.slots[to_index], max_stack, count);
        from.slots[from_index] = from_slot;
        to.slots[to_index] = to_slot;
        state.inventory.inventories.insert(from_id.to_owned(), from);
        state.inventory.inventories.insert(to_id.to_owned(), to);
        return Ok(moved as i64);
    }
    // 跨背包自动落位：目标放不下的部分退回源背包（语义与历史一致）。
    let requested = count.unwrap_or(i64::from(source.count)).max(1) as u32;
    let removed = from.remove(&source.item_id, requested);
    let leftover = to.add(&source.item_id, removed, max_stack);
    if leftover > 0 {
        let _ = from.add(&source.item_id, leftover, max_stack);
    }
    state.inventory.inventories.insert(from_id.to_owned(), from);
    state.inventory.inventories.insert(to_id.to_owned(), to);
    Ok(removed as i64 - leftover as i64)
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
                    dynamic: false,
                    layer: 0xFF,
                    mask: 0xFF,
                    ccd: false,
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
    let _ = state.physics.world2d.remove_body(entity);
    let Some(tilemap) = state.world.get::<Tilemap>(entity).ok().flatten() else {
        return;
    };
    if tilemap.solid.iter().any(|solid| *solid) {
        let colliders = solid_cell_colliders(tilemap);
        let _ = state
            .physics
            .world2d
            .add_body(entity, position, false, colliders);
    }
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

/// P1-③：`ScriptState` 的重入借用失败从 **RefCell panic** 降级为 Lua 可捕获错误。
///
/// Lua API 闭包统一用 `try_borrow(_mut)` + 本函数收尾：万一 mlua 经 `__index` 等
/// 元方法重入 Rust，脚本收到一条带原因的 error，宿主进程不再整个崩掉。
fn reentrant_borrow(error: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::external(format!(
        "script state is already borrowed (reentrant Lua call?): {error}"
    ))
}

/// P2-③：Lua API 闭包的借用样板收敛（`Rc<RefCell<ScriptState>>` 的扩展方法）。
///
/// 270+ 个绑定共同遵循"借用状态 → 业务逻辑 → 返回"的形状；把借用 + 重入报错
/// 收进这两方法后，每个绑定只剩一行意图（`s.lua_borrow()?`），样板不再逐处复制。
trait LuaStateBorrow {
    fn lua_borrow(&self) -> LuaResult<std::cell::Ref<'_, ScriptState>>;
    fn lua_borrow_mut(&self) -> LuaResult<std::cell::RefMut<'_, ScriptState>>;
}

impl LuaStateBorrow for std::rc::Rc<std::cell::RefCell<ScriptState>> {
    fn lua_borrow(&self) -> LuaResult<std::cell::Ref<'_, ScriptState>> {
        self.try_borrow().map_err(reentrant_borrow)
    }
    fn lua_borrow_mut(&self) -> LuaResult<std::cell::RefMut<'_, ScriptState>> {
        self.try_borrow_mut().map_err(reentrant_borrow)
    }
}
/// 鼠标按钮名 → [`MouseButton`]（P1-④：与 `parse_key_strict` 同一严格度）。
///
/// 旧实现把任何不认识的名字静默映射成 `Other(0)`，拼错的按钮名与真实按钮无法
/// 区分。现在未知名报 Lua 错误；接受的写法与 `Binding::parse` 的 `mouse:*` 同源：
/// `left` / `right` / `middle` / `other:<序号>`，`mouse:` 前缀可省，大小写不敏感。
fn parse_mouse_button(button: &str) -> LuaResult<MouseButton> {
    let raw = button.trim();
    if raw.is_empty() {
        return Err(api_error("mouse button name must not be empty"));
    }
    let value = raw
        .strip_prefix("mouse:")
        .unwrap_or(raw)
        .to_ascii_lowercase();
    match value.as_str() {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        "middle" => Ok(MouseButton::Middle),
        other => other
            .strip_prefix("other:")
            .and_then(|index| index.parse::<u16>().ok())
            .map_or_else(
                || {
                    Err(api_error(format!(
                        "unknown mouse button: {raw}（可用 left/right/middle/other:<序号>）"
                    )))
                },
                |index| Ok(MouseButton::Other(index)),
            ),
    }
}

#[cfg(test)]
mod tests;
