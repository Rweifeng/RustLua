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
    installers: &rustlua_lua_api::Registry,
) -> LuaResult<()> {
    globals::register(lua, Rc::clone(&state))?;
    // 游戏时钟绑定已下沉到时钟域（P1 ⑤-5）：`clock` / `calendar` / `day` 三张表。
    {
        let clock = state.borrow().game_clock.clone();
        rustlua_gameplay::clock_lua::install(lua, &clock)?;
    }
    // 实体账本与 entity / health / tag / interact.query 已下沉到 ECS 域（P1 ⑤-13）。
    {
        let world = state.borrow().world.clone();
        let entities = state.borrow().entity_state.clone();
        rustlua_ecs::entity_lua::install(lua, &world, &entities)?;
    }
    // visibility 跟着渲染域组件走（P1 ⑤-13）：Visibility 是"哪些 pass 看得见"的渲染约定。
    {
        let world = state.borrow().world.clone();
        rustlua_render::visibility_lua::install(lua, &world)?;
    }
    // interact 表的最后三个键留在本层（要写 UI 覆盖层文本），表由 ECS 那一步建好。
    {
        let interact: Table = lua.globals().get("interact")?;
        interact_prompt::register(lua, interact, Rc::clone(&state))?;
    }
    // 可渲染组件的绑定已下沉到渲染域（P1 ⑤-10）：sprite / quad / mesh / light 四张表。
    {
        let renderable = state.borrow().renderable.clone();
        let world = state.borrow().world.clone();
        let project_root = state.borrow().project_root.clone();
        rustlua_render::renderable_lua::install(lua, &renderable, &world, &project_root)?;
    }
    // world_text 归 UI 域（WorldText 的排版规则住在 ustlua-ui，不拉 ender → ui 的边）。
    {
        let world = state.borrow().world.clone();
        rustlua_ui::world_text_lua::install(lua, &world)?;
    }
    // 变换绑定已下沉到 ECS 域（P1 ⑤-10）：transform / transform3d 两张表。
    {
        let world = state.borrow().world.clone();
        rustlua_ecs::transform_lua::install(lua, &world)?;
    }
    // 相机绑定已下沉到相机域（P1 ⑤-8）：`camera` / `camera3d` / `camera2d` 三张表。
    {
        let camera = state.borrow().camera.clone();
        let world = state.borrow().world.clone();
        rustlua_camera::camera_lua::install(lua, &camera, &world, installers.pointer_cell())?;
    }
    // 瓦片地图绑定已下沉到物理域（P1 ⑤-14）：tilemap 改固态格要当帧重建静态刚体。
    {
        let world = state.borrow().world.clone();
        let entities = state.borrow().entity_state.clone();
        let physics = state.borrow().physics.clone();
        rustlua_physics::tilemap_lua::install(lua, &world, &entities, &physics)?;
    }
    // grid 是纯网格换算（无状态），跟着网格数学留在 gameplay。
    rustlua_gameplay::grid_lua::install(lua)?;
    // 动画绑定已下沉到动画域（P1 ⑤-10）：nimation / nimation3d 两张表。
    {
        let animation = state.borrow().animation.clone();
        let world = state.borrow().world.clone();
        rustlua_animation::animation_lua::install(lua, &animation, &world)?;
    }
    // 物理绑定已下沉到物理域（P1 ⑤-9）：`physics` / `physics3d` 两张表。
    {
        let physics = state.borrow().physics.clone();
        let world = state.borrow().world.clone();
        rustlua_physics::physics_lua::install(lua, &physics, &world)?;
    }
    // `particles` 留在脚本层：粒子是 `rustlua-ecs` 的发射器实例列表，没有领域规则要搬。
    particles::register(lua, Rc::clone(&state))?;
    world_data::register(lua, Rc::clone(&state))?;
    // prefab 的 Lua 表面在 UI 域（P1 ⑤-15），实现由本层的组合根注入：
    // 运行期实例化要按 &mut ScriptState 装配 18 类组件，领域 crate 不能反向依赖本层。
    rustlua_ui::prefab_lua::install(
        lua,
        Rc::new(crate::script::prefab_host::PrefabOps::new(Rc::clone(
            &state,
        ))),
    )?;
    // `ui` 表由三段共享：mlua 的 `Table` 是引用语义，克隆只是多一个句柄。
    let ui = lua.create_table()?;
    // `ui` 表由三段共享（P1 ⑤-12：`ui` / `ui_panel` 已下沉到 UI 域，本层只剩 `measure_text`）。
    {
        let ui_state = state.borrow().ui_state.clone();
        rustlua_ui::ui_lua::install(lua, ui.clone(), &ui_state)?;
        rustlua_ui::ui_panel_lua::install(lua, ui.clone(), &ui_state)?;
    }
    {
        let nav = state.borrow().nav.clone();
        rustlua_ui::lua_nav::install(lua, &ui, &nav)?;
    }
    // M76-07：虚拟列表 / 富文本 / 文本输入 / 导航栈四个高级控件内核。
    {
        let ui_state = state.borrow().ui_state.clone();
        rustlua_ui::lua_advanced::install(lua, &ui, &ui_state)?;
    }
    ui_measure::register(lua, ui.clone(), Rc::clone(&state))?;
    lua.globals().set("ui", ui)?;
    // 背包绑定已下沉到物品域（P1 ⑤-6）：`item` / `inv` / `container` / `shop` 四张表。
    {
        let inventory = state.borrow().inventory.clone();
        rustlua_gameplay::inventory_lua::install(lua, &inventory)?;
    }
    // 存档绑定已下沉到存档域（P1 ⑤-2）
    // `rustlua-gameplay` 提供安装器，本 crate 只把能力单元交出去。
    {
        let store = state.borrow().save.clone();
        rustlua_gameplay::save_lua::install(lua, &store)?;
    }
    installers.install(lua)?;
    Ok(())
}
