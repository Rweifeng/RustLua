//! Lua API 的分模块注册（WP11 阶段 2，由原 `register_api` 一整块拆出）。
//!
//! 每个子模块一个 `register(lua, state)`，把自己负责的 Lua 全局模块表装进 `lua`。
//! 模块表名字互不相同、且注册期不互相读取，所以子模块之间的先后不影响行为。
//! 唯一的例外是 `ui`：它有 45 个函数，按职责拆成 `ui` / `ui_panel` / `ui_nav` 三段共享同一张
//! 表——表在本文件建好、三段填完，最后才 `globals().set("ui", …)`。

use std::{cell::RefCell, rc::Rc};

use mlua::{Lua, Result as LuaResult, Table};

use super::ScriptState;

mod globals;
mod interact_prompt;
mod particles;
mod render_tuning;
mod ui_measure;
mod world_data;

/// 装配全部 Lua 全局模块表。`LuaRuntime` 每建一个 VM 调用一次。
///
/// 内建模块装完之后再装 `installers`：那是**领域 crate 提供**的绑定（见
/// [`rustlua_lua_api::Registry`]），组合根负责填充，本 crate 只认注册表不认具体 crate。
/// 顺序上放在最后，是为了让内建模块（`ui` 那张共享表等）先就位——外部模块可以依赖它们的
/// 存在，反过来不行。
pub(super) fn register_all(
    lua: &Lua,
    state: Rc<RefCell<ScriptState>>,
    installers: &crate::script::domain::lua_api::Registry,
) -> LuaResult<()> {
    globals::register(lua, Rc::clone(&state))?;
    // 游戏时钟绑定已下沉到时钟域（P1 ⑤-5）：`clock` / `calendar` / `day` 三张表。
    {
        let clock = state.borrow().game_clock.clone();
        crate::script::domain::gameplay::clock_lua::install(lua, &clock)?;
    }
    // 实体账本与 entity / health / tag / interact.query 已下沉到 ECS 域（P1 ⑤-13）。
    {
        let world = state.borrow().world.clone();
        let entities = state.borrow().entity_state.clone();
        crate::script::domain::ecs::entity_lua::install(lua, &world, &entities)?;
    }
    // visibility 跟着渲染域组件走（P1 ⑤-13）：Visibility 是"哪些 pass 看得见"的渲染约定。
    {
        let world = state.borrow().world.clone();
        crate::script::domain::render::visibility_lua::install(lua, &world)?;
    }
    // interact 表的最后三个键留在本层（要写 UI 覆盖层文本），表由 ECS 那一步建好。
    {
        let interact: Table = lua.globals().get("interact")?;
        interact_prompt::register(lua, interact, Rc::clone(&state))?;
    }
    // 可渲染组件的绑定已下沉到渲染域（P1 ⑤-10）：sprite / quad 两张基底表。
    {
        let renderable = state.borrow().renderable.clone();
        let world = state.borrow().world.clone();
        let project_root = state.borrow().project_root.clone();
        crate::script::domain::render::renderable_lua::install(
            lua,
            &renderable,
            &world,
            &project_root,
        )?;
        // M81：mesh / light / terrain3d / water3d 属 3D 包，2D 构建不登记。
        #[cfg(feature = "3d")]
        crate::script::domain::render::renderable_lua::install_3d(
            lua,
            &renderable,
            &world,
            &project_root,
        )?;
    }
    // M82-B2：`texture` 程序化贴图属维度无关基底（2D/3D 都登记）；from_text 要工程根解析字体。
    {
        let texture_gen = state.borrow().texture_gen.clone();
        let project_root = state.borrow().project_root.clone();
        crate::script::domain::render::texture_lua::install(lua, &texture_gen, &project_root)?;
    }
    // world_text / prefab / ui_lua / ui_panel / ui_controls / lua_nav / lua_advanced 七处绑定
    // 已下沉到 UI 域（docs/62 P1 阶段 4d）：能力单元由组合根持有，经延迟安装器接到脚本层
    // （见 `engine::host::script_packs`）。本层只建 `ui` 表并装 `measure_text`（要读脚本层的
    // 字体探针），其余键由组合根在**现场取到这张表后**补齐。
    // 变换绑定已下沉到 ECS 域（P1 ⑤-10）：transform 基底表（按组件分派，不按维度裁）。
    {
        let world = state.borrow().world.clone();
        crate::script::domain::ecs::transform_lua::install(lua, &world)?;
        // M81：transform3d 属 3D 包，2D 构建不登记。
        #[cfg(feature = "3d")]
        crate::script::domain::ecs::transform_lua::install_3d(lua, &world)?;
    }
    // 相机绑定（`camera` / `camera2d` / `camera3d`）不再由本层装：能力单元与绑定一起住在
    // `rustlua-camera`，由组合根经 [`LuaRuntime::add_deferred_installer`] 登记（见 `engine`
    // 的 `host::script_packs`）。本 crate 因此不再 `use rustlua_camera`——它只提供"什么时候
    // 可以装"（VM 与内建模块就位之后）和"世界在哪"（安装器的第一个参数）。
    // grid 是纯网格换算（无状态），跟着网格数学留在 gameplay。
    crate::script::domain::gameplay::grid_lua::install(lua)?;
    // `particles` 留在脚本层：粒子是 `rustlua-ecs` 的发射器实例列表，没有领域规则要搬。
    particles::register(lua, Rc::clone(&state))?;
    render_tuning::register(lua, Rc::clone(&state))?;
    world_data::register(lua, Rc::clone(&state))?;
    // `ui` 表由建表、本层的 `measure_text`、组合根的其余绑定三方共享；mlua 的 `Table` 是
    // 引用语义，克隆只是多一个句柄。**每次建 VM 都新建这张表并 set 到 globals**，组合根的
    // 延迟安装器因此必须在回调内现场 `globals().get("ui")` 取当次的表，绝不能捕获旧表句柄
    // （否则热重载/多 VM 路径下 `ui.image` 等键会装进旧表，脚本看到 nil）。
    let ui = lua.create_table()?;
    ui_measure::register(lua, ui.clone(), Rc::clone(&state))?;
    lua.globals().set("ui", ui)?;
    // 背包绑定已下沉到物品域（P1 ⑤-6）：`item` / `inv` / `container` / `shop` 四张表。
    {
        let inventory = state.borrow().inventory.clone();
        crate::script::domain::gameplay::inventory_lua::install(lua, &inventory)?;
    }
    // 存档绑定已下沉到存档域（P1 ⑤-2）
    // `rustlua-gameplay` 提供安装器，本 crate 只把能力单元交出去。
    {
        let store = state.borrow().save.clone();
        crate::script::domain::gameplay::save_lua::install(lua, &store)?;
    }
    installers.install(lua)?;
    Ok(())
}
