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
mod prefab_host;
mod progress;
mod require;
mod runtime;
mod scene_spawn;
mod step;
mod transition;
mod values;

// 跨 crate 的公开面（`editor` / `game_web` / `host` 用到）：消费方沿用的 `rustlua_scripting::script::*`
// 由 engine 的 core.rs 门面再导出，路径与拆分前一字不变。
pub use error::{LuaBridgeError, ScriptDebugger, ScriptDiagnostic};
pub use guard::{
    BudgetSlot, CallbackBudget, ContainOutcome, DEFAULT_CALLBACK_ERROR_LIMIT, EntityHook,
    ScriptErrorPolicy, ScriptGuard,
};
pub use progress::{DEFAULT_INSTRUCTION_BUDGET, ScriptProgress, script_progress};
pub use require::{install_web_require, read_project_script};
pub use values::entity_lua_id;

// WP11-USE:BEGIN
use error::ScriptFingerprint;
use rustlua_animation::animation::AnimationFrameEvent;
use rustlua_camera::camera::{Camera2D, Camera3D};
use rustlua_ecs::{
    entity::EntityId,
    particle::{Particle, ParticleEmitter, ParticleSystem},
    world::World,
};
use rustlua_gameplay::gameplay::GameClock;
use rustlua_gameplay::save_store::SaveStore;
use rustlua_lua_api::{Handle, ProjectRoot};
use rustlua_math::math2d::Vec2;
#[allow(unused_imports)] // 经 `script::tests` 的 `use super::*` 链供 `tests/camera.rs` 使用
use rustlua_math::math3d::Vec3;
use rustlua_scene::scene::SceneEntityId;
use rustlua_ui::ui::{DynamicUiImage, UiGrid, UiPanel};
use values::{format_lua_value, lua_value_to_json};
// `ui` 的运行期元素类型跟着状态单元搬去了 UI 域（P1 ⑤-12），路径保持不变以便宿主继续用。
pub use rustlua_ui::ui_state::{UiBar, UiDrag, UiText};
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

// 测试专用：反向名索引的重建次数。契约是「改名才重建、稳态反查零重建」，而重建产出的表与
// 重建前逐项相同——从外部观察不到，只能数次数（与 `rustlua-ecs` 的 `ORDER_REBUILDS` 同一手法）。
// 用 thread_local：`cargo test` 并行跑用例，全局计数会互相污染。

#[cfg(test)]
fn entity_name_rebuilds() -> u32 {
    rustlua_ecs::entity_state::entity_name_rebuilds()
}

// 只在测试里用到（经 `script::tests` 的 `use super::*` 链）：用 `cfg(test)` 收口，
// 免得给生产构建留一个 `#[allow(unused_imports)]` 的口子。
#[cfg(test)]
use rustlua_ecs::tilemap::Tilemap;
#[cfg(test)]
use rustlua_ecs::transform::Transform;

/// 运行期 spawn 的 UI 预设布局节点表 + **修订号**。
///
/// 宿主每帧都要先回答"这张表变没变"，才能决定要不要把节点并进自己的场景表。原来的做法是
/// **整表 clone 出来再 `==`**（`docs/60` §10.3 第 2 条）：表没变时白白复制一份，而表一旦非空
/// ——UI 预设化之后 `prefab.spawn` 会往里塞每个面板的布局节点——这份拷贝就**每帧都付**，
/// 而且比较本身也是 O(n)。把修订号与表放进**同一个类型**、改动只能经下面这几个方法，
/// **修订号不可能忘记自增**（与 `EntityNames` 的反查缓存同一个理由：靠人记必然漂移）。
///
/// 修订号单调递增，**不做"内容没变就不加"的优化**：多一次同步只是多付一次合并，漏一次同步
/// 却会让画面与命中框停在旧版上——两者代价不对称，所以宁多勿漏。
#[derive(Debug, Default)]
struct PrefabUiNodes {
    nodes: BTreeMap<u64, rustlua_ui::ui_scene_layout::UiSceneNode>,
    revision: u64,
}

impl PrefabUiNodes {
    fn clear(&mut self) {
        self.nodes.clear();
        self.revision = self.revision.wrapping_add(1);
    }

    fn remove(&mut self, key: &u64) {
        self.nodes.remove(key);
        self.revision = self.revision.wrapping_add(1);
    }

    fn extend(&mut self, nodes: BTreeMap<u64, rustlua_ui::ui_scene_layout::UiSceneNode>) {
        self.nodes.extend(nodes);
        self.revision = self.revision.wrapping_add(1);
    }

    fn revision(&self) -> u64 {
        self.revision
    }

    /// 给宿主的整表快照（宿主只在修订号变了之后才来取，所以这里 clone 一次是应该付的）。
    fn snapshot(&self) -> BTreeMap<u64, rustlua_ui::ui_scene_layout::UiSceneNode> {
        self.nodes.clone()
    }
}

#[derive(Debug, Default)]
pub(crate) struct ScriptState {
    delta_seconds: f32,
    quit_requested: bool,
    /// 世界的能力单元（P1 ⑤-7：ECS 世界从 `ScriptState` 的内联字段变成可共享的句柄）。
    ///
    /// 这一步**只换借用面**，不搬绑定位置：`state.world()` / `state.world_mut()` 是调用点的
    /// 短写（字段与同名方法在 Rust 里是两个命名空间）。宿主仍走 `with_world`/`with_world_mut`，
    /// 绑定仍在 `script/api/*`——等 `transform`/`entity` 这些模块搬到 owning crate 时，
    /// 它们拿的是同一个 `Handle<World>`，不需要 VM 宿主当中间人。
    world: Handle<World>,
    scene: Option<String>,
    scene_pending_load: Option<String>,
    /// 工程根的能力单元（P1 ⑤-4c）：绑定（`data.*` 等）与宿主读同一份，
    /// 因此运行期 `set_project_root` 对已装好的绑定立即生效（不需要重装 VM）。
    project_root: Handle<ProjectRoot>,
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
    /// 世界数据域的能力单元（P1 ⑤-4c：状态与绑定都归 `rustlua-gameplay`）。
    world_data: Handle<rustlua_gameplay::world_data_state::WorldDataState>,
    /// 相机域的能力单元（P1 ⑤-8：选中项 / 2D 控制器 / 两份每帧快照与三张 Lua 表都归
    /// `rustlua-camera`，绑定见 `rustlua_camera::camera_lua`）。
    camera: Handle<rustlua_camera::camera_state::CameraState>,
    particles: ParticleSystem,
    /// 动画域能力单元（P1 ⑤-10）：帧事件队列归动画域（rustlua-animation）。
    animation: Handle<rustlua_animation::animation_lua::AnimationState>,
    /// 渲染域的脚本可见状态（P1 ⑤-10：本帧主光的选择归 `rustlua-render`）。
    renderable: Handle<rustlua_render::renderable_lua::RenderableState>,
    /// 游戏时钟的能力单元（P1 ⑤-5：时钟类型与推进规则归 `rustlua-gameplay`，
    /// 绑定见 `rustlua_gameplay::clock_lua`，宿主推进见 [`LuaRuntime::step_clock`]）。
    game_clock: Handle<GameClock>,
    /// 运行期 `prefab.spawn` 的活实例（handle → 活世界落点）。despawn 与按名查节点都走
    /// 这张表；刻意不进场景簿记——切图不该把动态 UI 实例当场景实体重生（见
    /// `scene_spawn::PrefabSpawn` 头注释）。
    prefab_spawns: HashMap<i64, scene_spawn::PrefabSpawn>,
    next_prefab_handle: i64,
    /// 运行期 spawn 出去的 UI 预设实体的**布局节点**（键 = 活实体槽位号）。
    /// 宿主每帧解算场景 UI 矩形时把这张表并进自己的快照，于是"画得出"与"点得中"
    /// 对 spawn 的实体同样成立；场景加载那一份走同一个 builder（见
    /// `rustlua_ui::ui_scene_layout::build_ui_scene_nodes`）。
    ///
    /// 表与**修订号**封在一个类型里（[`PrefabUiNodes`]）：宿主先比修订号，没变就**一次 clone 都不做**
    /// ——原来的写法是每帧整表 clone 再 `==`，UI 预设化之后那张表是非空的，代价每帧都付。
    prefab_ui_nodes: PrefabUiNodes,
    /// docs/54 §8-②：最近一次场景加载里**坏的 `parent` 边**（悬空父/自环）的可读描述。
    /// 运行时会把这类边当成"没有父"（容错），因此必须同时可发现，否则子树会静默少一层位移。
    scene_parent_issues: Vec<String>,
    /// docs/54 §4.3：一个实体挂了多种可绘制组件的点名（渲染只按优先级画一种，但没有诊断的话
    /// "为什么另一个没画出来"无从解释）。
    scene_renderable_conflicts: Vec<String>,
    // ---- docs/60 §4.1：按领域拆分的子状态 ----
    /// 实体账本的能力单元（P1 ⑤-13：名字/持久 id/血量/标签/场景声明表归 `rustlua-ecs`）。
    entity_state: Handle<rustlua_ecs::entity_state::EntityState>,
    /// UI 域的能力单元（P1 ⑤-12：动态文本/条/面板/图/网格/拖拽/视口都归 `rustlua-ui`）。
    ui_state: Handle<rustlua_ui::ui_state::UiState>,
    /// 物理模拟域的能力单元（P1 ⑤-9：两个世界、分阶段回写队列与调试请求都归
    /// `rustlua-physics`，绑定见 `rustlua_physics::physics_lua`）。
    physics: Handle<rustlua_physics::physics_state::PhysicsState>,
    /// 物品/背包/商店域的能力单元（P1 ⑤-6：数据与槽位规则归 `rustlua-gameplay`）。
    inventory: Handle<rustlua_gameplay::inventory_state::InventoryState>,
    /// 存档域的能力单元（P1 ⑤：数据/路径/槽位归 `rustlua-gameplay`）。
    save: Handle<SaveStore>,
    /// UI 导航的能力单元（P1 ⑤-3：焦点环/滚动位置归 `rustlua-ui`）。
    nav: Handle<rustlua_ui::lua_nav::NavState>,
}

impl ScriptState {
    /// 只读借用世界（P1 ⑤-7）。调用点写 `state.world()?.get::<T>(id)`——比
    /// `state.world.borrow()?` 短，且不会把 `Handle` 的字段名带进每个绑定。
    fn world(&self) -> LuaResult<std::cell::Ref<'_, World>> {
        self.world.borrow()
    }

    /// 可写借用世界（P1 ⑤-7）。
    fn world_mut(&self) -> LuaResult<std::cell::RefMut<'_, World>> {
        self.world.borrow_mut()
    }

    /// 懒加载字体度量探针（`ui.measure_text`）：要工程根才能找到 `project.toml`
    /// 里的字体配置，故第一次调用才读盘；失败结果也会缓存（`Some(None)`），
    /// 避免每帧重复尝试读一个不存在的字体。切工程（`set_project_root`）会清缓存。
    fn font_probe(&mut self) -> Result<Option<rustlua_render::font::FontProbe>, LuaBridgeError> {
        if let Some(cached) = &self.font_probe {
            return Ok(cached.clone());
        }
        let root = self
            .project_root
            .with(|root| root.path().map(Path::to_path_buf))
            .ok_or_else(|| LuaBridgeError::Api("no project root configured".to_owned()))?;
        // 这一步 = 读工程字体 + 建度量探针（Web 上还要先解包），是 `ui.measure_text` 的**首次**
        // 调用成本。第一帧卡顿的归因全靠这个读数：把它记成一条日志（wasm 侧经 `wasm_log` 进
        // 控制台），就不必再靠"首帧总耗时"反推到底是字体、脚本还是渲染。
        let started = web_time::Instant::now();
        let probe = rustlua_render::font::project_font::load_probe(&root).ok();
        tracing::info!(
            target: "render",
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            ok = probe.is_some(),
            root = %root.display(),
            "[render] font probe loaded"
        );
        self.font_probe = Some(probe.clone());
        Ok(probe)
    }
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
    /// 领域 crate 提供的 Lua 模块（依赖倒置的落点，见 [`LuaRuntime::with_installers`]）。
    ///
    /// 用 `Rc` 持有：热重载会重建 VM，每个新 VM 都要装同一套安装器；安装器本身捕获的
    /// 是 `rustlua-lua-api` 的 `Handle`（宿主能力单元），所以注册表可以被多个 VM 共享。
    installers: Rc<rustlua_lua_api::Registry>,
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
            let _ = rustlua_lua_api::Registry::new();
        }
        _ => return Err("unknown stage".to_owned()),
    }
    Ok(())
}

impl LuaRuntime {
    pub fn set_delta(&mut self, delta_seconds: f32) {
        let delta_seconds = delta_seconds.max(0.0);
        self.state.borrow_mut().delta_seconds = delta_seconds;
        // 下沉后的 `input` 绑定靠它把逻辑时间按固定步推进（连发节奏与帧率无关）。
        self.installers.set_delta_seconds(delta_seconds);
    }

    /// 读 2D 相机控制器（WP4）。相机数据本身仍在宿主侧，这里只放"center 该怎么算"。
    ///
    /// P1 ⑤-8 之后控制器住在 `Handle<CameraState>` 里；宿主侧签名保持不变（引擎与编辑器
    /// 的 7 处调用一字未改），重入由 [`Handle::with`] 的命名 panic 兜底。
    pub fn camera2d_controller<R>(
        &self,
        visit: impl FnOnce(&rustlua_camera::camera::Camera2dController) -> R,
    ) -> R {
        let camera = self.state.borrow().camera.clone();
        camera.with(|state| visit(state.controller()))
    }
    /// 改 2D 相机控制器（Lua `camera2d.*` 与宿主每帧推进共用）。
    pub fn camera2d_controller_mut<R>(
        &mut self,
        visit: impl FnOnce(&mut rustlua_camera::camera::Camera2dController) -> R,
    ) -> R {
        let camera = self.state.borrow().camera.clone();
        camera.with_mut(|state| visit(state.controller_mut()))
    }
    /// Stores the current viewport so Lua `ui.grid_hit` can resolve screen
    /// coordinates. The host calls this before rendering each frame.
    pub fn set_ui_viewport(&mut self, width: u32, height: u32) {
        let ui = self.state.borrow().ui_state.clone();
        ui.with_mut(|ui| ui.set_viewport(width, height));
    }

    /// Stores the Canvas 空间 UI 参考分辨率（设计分辨率），供 Lua 侧
    /// `process_ui_clicks` / `ui.grid_hit` 命中测试与渲染保持一致。
    pub fn set_ui_reference(&mut self, reference: [f32; 2]) {
        let ui = self.state.borrow().ui_state.clone();
        ui.with_mut(|ui| ui.set_reference(reference));
    }

    /// Sets the project root, used to resolve `scenes/<name>.scene` on load.
    ///
    /// 同时把**生效的设置根**写进注册表：`audio.load_settings` 等下沉后的设置持久化绑定读的是
    /// 那里（`docs/62` P1）。这不是重复状态——`settings_root` 是运行时的口径（`input` 绑定还在用），
    /// 注册表里的是"领域绑定能看见的那一份"，两者由这两个 setter 保持同步。
    pub fn set_project_root(&mut self, root: PathBuf) {
        // P1-③：一次借用完成两件事——相邻的两次 `borrow_mut()` 合并为一个借用
        // 作用域，杜绝任何"两次借用之间被重入"的窗口。
        self.publish_settings_root(root.clone());
        let mut state = self.state.borrow_mut();
        state.project_root.with_mut(|project| project.set(root));
        // 换工程 → 字体也可能换：丢缓存，下次 `ui.measure_text` 重新读盘。
        state.font_probe = None;
    }

    /// Sets the file path save data is committed to / loaded from.
    pub fn set_save_path(&mut self, path: PathBuf) {
        self.state
            .borrow()
            .save
            .with_mut(|save| save.set_path(Some(path)));
    }

    /// 设置玩家设置根目录（`settings.toml` 的读写根）。
    /// 发布包由宿主指向用户数据目录（见 `rustlua_infra::user_dir`），开发工程不调用，
    /// 于是退回项目根——编辑器与 `jhyt_m20_*` 检查的就是工程内那份。
    pub fn set_settings_root(&mut self, root: PathBuf) {
        self.publish_settings_root(root.clone());
        self.state.borrow_mut().settings_root = Some(root);
    }

    /// 把"当前生效的设置根"同步给注册表（领域绑定经它读 `settings.toml`）。
    ///
    /// 口径与 [`settings_root`] 一致：显式 `settings_root` 优先，否则退回项目根。显式值还没设时
    /// 先把项目根写进去，后续 `set_settings_root` 会覆盖。
    fn publish_settings_root(&self, root: PathBuf) {
        self.installers.set_root(Some(root));
    }

    /// Gives the host a read view of the UI text and bar elements.
    pub fn with_ui<R>(
        &self,
        f: impl FnOnce(&BTreeMap<String, UiText>, &BTreeMap<String, UiBar>) -> R,
    ) -> R {
        let ui = self.state.borrow().ui_state.clone();
        ui.with(|ui| f(ui.texts(), ui.bars()))
    }

    /// 场景文字按名隐藏集（`ui.scene_text_visible` 写入）的只读视图。
    #[must_use]
    pub fn scene_text_hidden(&self) -> BTreeSet<String> {
        let ui = self.state.borrow().ui_state.clone();
        ui.with(|ui| ui.scene_text_hidden().clone())
    }

    /// P3-②：一次借用判定"实体的名字是否被隐藏"——此前宿主每帧为每个文字
    /// `scene_text_hidden().clone()` 整份隐藏集再 `contains`。
    #[must_use]
    pub fn is_entity_scene_text_hidden(&self, id: EntityId) -> bool {
        let state = self.state.borrow();
        let ui = state.ui_state.clone();
        let hidden = ui.with(|ui| ui.scene_text_hidden().clone());
        let entities = state.entity_state.clone();
        // 只扫**这个实体自己的**名字（原来是扫全表每一条），"任一名字命中即隐藏"的语义原样保留。
        entities.with(|entities| {
            entities
                .names
                .any_name_matches(id, |name| hidden.contains(name))
        })
    }

    /// 运行期 spawn 的 UI 预设实体的布局节点（键 = 活实体槽位号）。宿主每帧解算场景
    /// UI 矩形前把它并进自己的快照，spawn 出来的 UI 才既画得出也点得中。
    ///
    /// 返回**克隆**而不是引用：`RefCell<ScriptState>` 借用期间脚本不能再借用，而宿主
    /// 解算布局时还要 `with_world`（同一个 state 的另一个字段）。
    ///
    /// **调用方应当先看 [`Self::prefab_ui_nodes_revision`]**：修订号没变就跳过这一次克隆。
    /// （原来这里写着"空表 = 不付额外拷贝成本"——**那句话是错的**：宿主是**先**调本方法拿到克隆、
    /// **再**比较是否为空，所以空表也要先复制一次；宿主侧已改成先比修订号。）
    #[must_use]
    pub fn prefab_ui_nodes(&self) -> BTreeMap<u64, rustlua_ui::ui_scene_layout::UiSceneNode> {
        self.state.borrow().prefab_ui_nodes.snapshot()
    }

    /// [`Self::prefab_ui_nodes`] 的修订号：**只在表真的被动过之后才变**。
    /// 宿主每帧先比这个数，未变即跳过整表克隆与 `==` 比较（见 `docs/60` §10.3 第 2 条）。
    #[must_use]
    pub fn prefab_ui_nodes_revision(&self) -> u64 {
        self.state.borrow().prefab_ui_nodes.revision()
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

    /// 「名字 → **引擎句柄**」，按 Lua 侧编码（`index << 32 | generation`）返回；查不到或那个
    /// 名字指向的实体已经不在世 ⇒ `None`。
    ///
    /// 存在的理由：webgl/微信的 JS 宿主有自己的一套 Lua 世界（`web-runtime.js` 的 `entity` 表覆盖了
    /// 引擎的同名 API），它用 **`.scene` 文件里的作者 id** 当实体 id。于是同一份工程里出现两个 id 空间，
    /// 一旦某个 API 没被 JS 宿主覆盖（`quad` 就是），作者 id 就会以**引擎句柄**的身份被解包：
    /// 第 60 轮实测到 `entity\t13\tScaleBar` 的作者 id `13` 被解码成 `EntityId(0:g13)`
    /// （`index = 13 >> 32 = 0`、`generation = 13`）⇒ `invalid entity id`。
    ///
    /// 有了它，JS 宿主可以在**生成 Lua 绑定表**时先问引擎要真句柄，两个世界就从"各说各话"变成
    /// "以引擎为准"；引擎不认识这个名字时给出 `None`（调用方退回 `nil`），而不是一个用不了的数字。
    #[must_use]
    pub fn entity_handle_lua(&self, name: &str) -> Option<i64> {
        let state = self.state.borrow();
        let entities = state.entity_state.clone();
        let handle = entities.with(|entities| entities.names.get(name))?;
        // 与 `entity.find` 同一条教条：只交活着的句柄。
        // （重入失效时按"不在世"处理：宿主拿 `None` 退回 `nil`，比 panic 好。）
        state
            .world()
            .ok()
            .is_some_and(|world| world.is_alive(handle))
            .then(|| values::entity_to_lua(handle))
    }

    /// 「活实体槽位号 → 名字」的全量登记（`entity.find` 查的就是这张表，所以场景实体与
    /// 运行期 `prefab.spawn` 的实例都在里头）。宿主把解算出的 UI 矩形换名报回时用它——
    /// 同一份黄金表因此能跨"载体是 `.scene` 还是 `.prefab`"对账。
    #[must_use]
    pub fn entity_names_by_slot(&self) -> BTreeMap<u64, String> {
        let entities = self.state.borrow().entity_state.clone();
        entities.with(|entities| {
            entities
                .names
                .iter_by_name()
                .map(|(name, handle)| (handle.index() as u64, name.clone()))
                .collect()
        })
    }

    /// [`Self::entity_names_by_slot`] 的**带代际**版本：给出完整句柄而不是槽位号。
    ///
    /// 编辑器 Play 期间要列运行态实体树，那里"槽位复用"是常态（子弹每秒生成/销毁），
    /// 只给槽位号会把复用槽位上的新实体认成旧实体。
    #[must_use]
    pub fn named_entity_handles(&self) -> Vec<(EntityId, String)> {
        let entities = self.state.borrow().entity_state.clone();
        entities.with(|entities| {
            entities
                .names
                .iter_by_name()
                .map(|(name, handle)| (*handle, name.clone()))
                .collect()
        })
    }

    /// 「场景实体 id → 运行时句柄」的完整登记表。
    ///
    /// 宿主据此判断镜像里的哪一行对应世界里的哪个实体（含反向：某个句柄是不是场景
    /// 声明的，而不是脚本运行期 spawn 的）。
    #[must_use]
    pub fn scene_entity_handles(&self) -> Vec<(SceneEntityId, EntityId)> {
        let entities = self.state.borrow().entity_state.clone();
        entities.with(|entities| {
            entities
                .scene_entities
                .iter()
                .map(|(id, handle)| (*id, *handle))
                .collect()
        })
    }

    /// 场景声明的实体数（不含脚本运行期 spawn 的实体）。性能面板的常驻读数用，
    /// 不分配登记表；需要逐条句柄时用 [`Self::scene_entity_handles`]。
    #[must_use]
    pub fn scene_entity_count(&self) -> usize {
        let entities = self.state.borrow().entity_state.clone();
        entities.with(|entities| entities.scene_entities.len())
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
        let ui = self.state.borrow().ui_state.clone();
        ui.with(|ui| f(ui.panels(), ui.images(), ui.grids()))
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
        let handle = self.state.borrow().camera.clone();
        handle.with_mut(|state| state.set_view_2d(camera, viewport));
    }

    /// docs/54 §8-⑦：宿主每帧把**3D pass 实际用的那台相机**与视口回填进来，供 Lua
    /// `camera3d.ray` / `camera3d.world_to_screen` 使用（与编辑器 picking 同一套投影函数）。
    pub fn set_camera3d_view(&mut self, camera: Camera3D, viewport: [f32; 2]) {
        let handle = self.state.borrow().camera.clone();
        handle.with_mut(|state| state.set_view_3d(camera, viewport));
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
        // 先取句柄副本再借世界：`f` 是宿主代码，若它回调进 Lua（`state.borrow_mut()`），
        // 手里还攥着 `Ref<ScriptState>` 就会撞重入错。
        let world = self.state.borrow().world.clone();
        world.with(f)
    }
    /// WP7 ②：取走 Lua 通过 `physics.debug_draw(...)` 登记的请求（取走即清空，与
    /// `camera2d` 的 pending 请求同一语义：请求只在被消费的那一帧生效）。
    pub fn take_collider_debug_request(&mut self) -> Option<bool> {
        // P1 ⑤-9：物理句柄与 `ScriptState` 是两处借用，先取句柄副本再动它（与 `with_world` 同法）。
        let physics = self.state.borrow().physics.clone();
        physics.with_mut(|physics| physics.take_debug_request())
    }
    /// WP4 ⑦：宿主推进动画后把本步触发的帧事件交进来，供 Lua `animation.events()` 轮询。
    pub fn push_animation_events(&mut self, events: Vec<(EntityId, AnimationFrameEvent)>) {
        let animation = self.state.borrow().animation.clone();
        animation.with_mut(|state| state.push_events(events));
    }
    /// 物理世界里当前所有 2D 碰撞体的姿态与形状，供宿主做 collider 调试绘制。
    pub fn collider_debug_shapes(&self) -> Vec<rustlua_physics::physics::ColliderDebugShape> {
        let physics = self.state.borrow().physics.clone();
        physics.with(|physics| physics.debug_shapes())
    }
    /// WP7 ②/docs/54 §8-⑥：取走 Lua 通过 `physics3d.debug_draw(...)` 登记的 3D 请求
    /// （与 2D 同一条"取走即清空"语义）。
    pub fn take_collider_debug_request_3d(&mut self) -> Option<bool> {
        let physics = self.state.borrow().physics.clone();
        physics.with_mut(|physics| physics.take_debug_request_3d())
    }
    /// 物理世界里当前所有 3D 碰撞体的姿态与形状，供宿主做 3D collider 调试绘制。
    pub fn collider_debug_shapes_3d(
        &self,
    ) -> Vec<rustlua_physics::physics3d::ColliderDebugShape3d> {
        let physics = self.state.borrow().physics.clone();
        physics.with(|physics| physics.debug_shapes_3d())
    }
    /// Gives the host a shared write view of the runtime entity world.
    pub fn with_world_mut<R>(&mut self, f: impl FnOnce(&mut World) -> R) -> R {
        let world = self.state.borrow().world.clone();
        world.with_mut(f)
    }

    /// Returns the live ECS handle created from an authored scene entity.
    #[must_use]
    pub fn scene_entity_handle(&self, id: SceneEntityId) -> Option<EntityId> {
        let entities = self.state.borrow().entity_state.clone();
        entities.with(|entities| entities.scene_entities.get(&id).copied())
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
    ///
    /// **同一实体挂多个名字时取 `by_name` 键序里的第一个**——这与改造前那条
    /// `iter().find(...)` 的结果逐字相同；变的只是复杂度（O(n) 线性扫 → O(1) 反查）。
    #[must_use]
    pub fn entity_name(&self, id: EntityId) -> Option<String> {
        let entities = self.state.borrow().entity_state.clone();
        entities.with(|entities| entities.names.first_name(id))
    }

    /// Returns the entity whose `Camera` the passes should use, if Lua selected one with
    /// `camera.set_active`（3D pass 只在它是透视相机时用它，2D 只在它是正交相机时用它）。
    #[must_use]
    pub fn active_camera(&self) -> Option<EntityId> {
        let handle = self.state.borrow().camera.clone();
        handle.with(|state| state.active())
    }

    /// Returns the entity whose `Light` the renderer should use, if Lua
    /// selected one with `light.set_active`.
    #[must_use]
    pub fn active_light(&self) -> Option<EntityId> {
        let renderable = self.state.borrow().renderable.clone();
        renderable.with(|state| state.active_light())
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
// 相机快照的取用（`camera_view_from` / `camera3d_view_from`）随绑定搬到了
// `rustlua_camera::camera_lua`（P1 ⑤-8）：那是"没渲染过一帧"这条口径的唯一实现点。
/// 工程里某个场景文件的路径（`<project_root>/scenes/<name>.scene`）。
pub(crate) fn scene_file_path(state: &ScriptState, name: &str) -> Option<PathBuf> {
    // 句柄只在这一次解析里借用：`resolve` 返回自有 `PathBuf`，guard 不跨调用。
    // 借用失败（只可能来自绑定重入）按"没有工程根"处理：场景解析是只读快路径，
    // 调用方（`scene_file_exists` → `map.goto`）本来就以 `false` 表示"切不了"。
    state
        .project_root
        .borrow()
        .ok()?
        .resolve(format!("scenes/{name}.scene"))
}

/// 场景文件是否存在。**必须经 [`rustlua_infra::vfs`]**：wasm 上没有文件系统，
/// 资源 provider 才是唯一真相，`Path::is_file()` 在那里恒假——后果是**每一次**
/// 场景切换被静默忽略（第 45 轮在 jhyt 的 webgl 产物上实测：标题界面确认后
/// `[map] scene file not found; load ignored name=manor`，画面停在 95.9% 单色的
/// 空白页，玩法系统一次都不跑）。桌面无 provider 时回落文件系统，行为不变。
pub(crate) fn scene_file_exists(state: &ScriptState, name: &str) -> bool {
    scene_file_path(state, name).is_some_and(|path| rustlua_infra::vfs::is_file(&path))
}

/// Queues a scene load when the scene file exists; mirrors `scene.load` so
/// `map.goto` can share the same validation without duplicating it.
fn request_scene_load(state: &ScriptState, name: &str) -> bool {
    let exists = scene_file_exists(state, name);
    if exists {
        tracing::info!(target: "scene", name = %name, "[map] scene transition queued");
    } else {
        tracing::warn!(target: "scene", name = %name, "[map] scene file not found; load ignored");
    }
    exists
}

/// P1-③：`ScriptState` 的重入借用失败从 **RefCell panic** 降级为 Lua 可捕获错误。
///
/// Lua API 闭包**应当**用 `try_borrow(_mut)` + 本函数收尾：万一 mlua 经 `__index` 等
/// 元方法重入 Rust，脚本收到一条带原因的 error，宿主进程不再整个崩掉（wasm/微信上 panic 是
/// 硬 abort，表现为裸 `unreachable`——这条降级是发布形态的刚需，不是洁癖）。
///
/// ⚠️ **2026-10-06 复核：这条"应当"只做到了 61%**。当时实测：`lua_borrow*()` 调用点 **173 处**，
/// 而直接的 `s.borrow_mut()` **63 处** + `s.borrow()` **46 处**（合计 109 处）仍是 panic 版借用。
/// 原措辞写的是"统一用"，与代码不符，已改正——**注释不该比代码承诺得多**。
/// 收口方向与验收办法见 `docs/60` §10 的 §4.3 条（要点：外层与内层都要转，只转一层等于没转；
/// 最好再用类型把 panic 版借用藏起来，免得下一个绑定又写回 `s.borrow_mut()`）。
fn reentrant_borrow(error: impl std::fmt::Display) -> mlua::Error {
    // 文案由契约层统一（`rustlua-lua-api`）：绑定下沉之后，"被借住的单元"不止这一个中枢，
    // 两处各写一句会让同一次重入在日志里长得不一样。
    rustlua_lua_api::reentrant_borrow("script state", error)
}

/// P2-③：Lua API 闭包的借用样板收敛（`Rc<RefCell<ScriptState>>` 的扩展方法）。
///
/// 270+ 个绑定多半遵循"借用状态 → 业务逻辑 → 返回"的形状；把借用 + 重入报错收进这两方法后，
/// 每个绑定只剩一行意图（`s.lua_borrow()?`），样板不再逐处复制。**覆盖率见上面的 2026-10-06 复核注**。
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
#[cfg(test)]
mod tests;
