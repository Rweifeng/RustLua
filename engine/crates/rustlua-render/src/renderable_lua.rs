//! 可渲染对象：`sprite` / `quad` / `mesh` / `light` / `world_text`（P1 ⑤-10 从脚本层搬来）。
//!
//! 组件类型都住在 `rustlua-render`，所以它们的 Lua 面也在这里：贴图/图集解析、网格路径、
//! 光源参数、世界文字排版都不再需要 VM 宿主当中间人。绑定只碰三样东西——`World`（组件读写）、
//! `RenderableState`（"哪盏灯是本帧的主光"，与相机选中项同构）、`ProjectRoot`（图集引用按
//! 工程根解析）。
//!
//! 注意 `sprite.get` 读的是 **`AnimationStateMachine` 的当前 clip sheet 优先、`SpriteAsset`
//! 兜底**：这条优先级本来就是渲染规则，搬进渲染域后它与 `frame_data.rs` 的读取同源。

use crate::atlas::resolve_atlas_reference;
use crate::light::Light;
use crate::mesh3d::MeshRenderer;
use crate::sprite::{SpriteAsset, SpriteComponent};
use mlua::{Lua, Result as LuaResult};
use rustlua_animation::animation::AnimationStateMachine;
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::{World, WorldError};
use rustlua_lua_api::{Handle, ProjectRoot, api_error};
use rustlua_math::math3d::Vec3;
use std::cell::{Ref, RefMut};

/// 渲染域里"跨帧的脚本可见状态"：本帧主光。
///
/// 与相机 `set_active` 同构：脚本只**登记选择**，渲染各 pass 决定怎么用它；场景切换让旧
/// 实体 id 失效（换图后旧句柄不该还点着上一张图的灯）。
#[derive(Debug, Default)]
pub struct RenderableState {
    active_light: Option<EntityId>,
}

impl RenderableState {
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "renderable")
    }

    #[must_use]
    pub fn active_light(&self) -> Option<EntityId> {
        self.active_light
    }

    pub fn set_active_light(&mut self, entity: EntityId) {
        self.active_light = Some(entity);
    }

    pub fn clear_for_scene_transition(&mut self) {
        self.active_light = None;
    }
}

/// 绑定侧的借用面：世界 + 渲染状态 + 工程根三个能力单元，加与脚本层同名的短写。
#[derive(Clone)]
struct Bindings {
    world: Handle<World>,
    state: Handle<RenderableState>,
    project_root: Handle<ProjectRoot>,
}

impl Bindings {
    fn new(
        world: &Handle<World>,
        state: &Handle<RenderableState>,
        project_root: &Handle<ProjectRoot>,
    ) -> Self {
        Self {
            world: world.clone(),
            state: state.clone(),
            project_root: project_root.clone(),
        }
    }

    fn world(&self) -> LuaResult<Ref<'_, World>> {
        self.world.borrow()
    }

    fn world_mut(&self) -> LuaResult<RefMut<'_, World>> {
        self.world.borrow_mut()
    }

    fn set_active_light(&self, entity: EntityId) -> LuaResult<()> {
        self.state.borrow_mut()?.set_active_light(entity);
        Ok(())
    }

    /// 工程根（未设置时 `None`，与脚本层的 `state.project_root.borrow()?.path()` 一致）。
    fn project_root_path(&self) -> LuaResult<Option<std::path::PathBuf>> {
        Ok(self
            .project_root
            .borrow()?
            .path()
            .map(std::path::Path::to_path_buf))
    }
}

/// `WorldError` → Lua 错误（每个领域绑定各留三行，避免把 `WorldError` 塞进叶子契约层）。
fn world_error(error: WorldError) -> mlua::Error {
    api_error(error.to_string())
}

/// Lua 侧实体 id → 实体句柄（错误串与脚本层逐字相同）。
fn entity_from_lua(value: i64) -> LuaResult<EntityId> {
    if value < 0 {
        return Err(api_error("invalid entity id: negative value"));
    }
    EntityId::from_lua_i64(value).ok_or_else(|| api_error(format!("invalid entity id: {value}")))
}

/// 写操作的前置校验：实体必须还活着。
fn require_alive(state: &Bindings, entity: EntityId) -> LuaResult<()> {
    if state.world()?.is_alive(entity) {
        Ok(())
    } else {
        Err(api_error(format!("invalid entity id: {entity}")))
    }
}
pub fn install(
    lua: &Lua,
    state: &Handle<RenderableState>,
    world: &Handle<World>,
    project_root: &Handle<ProjectRoot>,
) -> LuaResult<()> {
    let sprite = lua.create_table()?;
    let s = Bindings::new(world, state, project_root);
    sprite.set(
        "set",
        lua.create_function(move |_, (id, component): (i64, String)| {
            let id = entity_from_lua(id)?;
            let component = SpriteComponent::from_component_str(&component)
                .map_err(|error| api_error(error.to_string()))?;
            let sprite_value = component
                .sprite()
                .map_err(|error| api_error(error.to_string()))?;
            let state = &s;
            require_alive(state, id)?;
            let project_root = state.project_root_path()?;
            // 图集引用语法 `图集路径#区域名`（与 scene_spawn 同一约定），路径按项目根解析。
            let (texture_path, sprite_value) =
                match resolve_atlas_reference(project_root.as_deref(), &component.texture)
                    .map_err(|error| api_error(error.to_string()))?
                {
                    Some((texture_path, region)) => (
                        texture_path,
                        region
                            .apply(sprite_value)
                            .map_err(|error| api_error(error.to_string()))?,
                    ),
                    None => (component.texture.clone(), sprite_value),
                };
            state
                .world_mut()?
                .insert(id, sprite_value)
                .map_err(world_error)?;
            state
                .world_mut()?
                .insert(id, SpriteAsset(texture_path))
                .map_err(world_error)?;
            // 排序层必须与 `Sprite` 同批写入：渲染侧读的是独立的 `SpriteSorting` 组件
            // （`runtime_scene/frame_data.rs`），漏写就等于运行期声明的 `sorting_layer=` /
            // `order=` 全部失效，精灵落进"遍历序号"兜底桶。场景加载路径
            // （`script/scene_spawn.rs`）一直是写的，两条路径不一致时 web（直接解析组件串）
            // 与原生就会给出两套遮挡顺序。
            state
                .world_mut()?
                .insert(id, component.sorting.clone())
                .map_err(world_error)?;
            Ok(())
        })?,
    )?;
    // 运行期读回渲染用的贴图/尺寸/排序层：`sprite.set` 会同时写 Sprite、SpriteAsset 与
    // SpriteSorting 三个组件，验收驱动与工具需要能确认"实体上现在真的是哪张图、排在哪儿"
    // （M21 主角形象切换、NPC/敌人形象诊断），否则只能间接猜测。纹理解析顺序与渲染一致：
    // Animation 的当前 clip sheet 优先，没有动画时用 SpriteAsset。
    let s = Bindings::new(world, state, project_root);
    sprite.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let value = world
                .get::<crate::sprite::Sprite>(id)
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
                    .world()?
                    .get::<SpriteAsset>(id)
                    .map_err(world_error)?
                    .map(|asset| asset.0.clone()),
            )?;
            // 有动画时渲染走 clip 的 sheet（覆盖 Sprite 的 texture），
            // 一并以 animation_sheet 给出，避免调用方以为"贴图没换"。
            if let Some(machine) = state
                .world()?
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            {
                table.set("animation_sheet", machine.current_clip().sheet.clone())?;
            }
            // 排序层用组件串同名键回读（`sorting_layer` / `order`），脚本可以原样 round-trip。
            if let Some(sorting) = state
                .world()?
                .get::<crate::sprite::SpriteSorting>(id)
                .map_err(world_error)?
            {
                table.set("sorting_layer", sorting.layer.clone())?;
                table.set("order", sorting.order)?;
            }
            Ok(table)
        })?,
    )?;
    lua.globals().set("sprite", sprite)?;
    // docs/54 §4.3：一等 Quad 的运行时入口。与 `sprite` 分开是因为两者语义不同（Quad 没有纹理
    // 区域、不参与帧动画），而不是同一个东西的两种写法——脚本要"一个纯色矩形"时不该被迫写
    // `texture=__white__` 这种内部哨兵。

    let quad = lua.create_table()?;
    let s = Bindings::new(world, state, project_root);
    quad.set(
        "set",
        lua.create_function(move |_, (id, component): (i64, String)| {
            let id = entity_from_lua(id)?;
            let parsed = crate::quad::Quad::from_component_str(&component)
                .map_err(|error| api_error(error.to_string()))?;
            let state = &s;
            require_alive(state, id)?;
            state.world_mut()?.insert(id, parsed).map_err(world_error)?;
            Ok(())
        })?,
    )?;
    let s = Bindings::new(world, state, project_root);
    quad.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let value = world
                .get::<crate::quad::Quad>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no quad"))?;
            let table = lua.create_table()?;
            table.set("size_x", value.size.x)?;
            table.set("size_y", value.size.y)?;
            table.set("pivot_x", value.pivot.x)?;
            table.set("pivot_y", value.pivot.y)?;
            table.set("color_r", value.color[0])?;
            table.set("color_g", value.color[1])?;
            table.set("color_b", value.color[2])?;
            table.set("color_a", value.color[3])?;
            // 缺省贴图时给出实际会采样到的路径（内置白纹理哨兵），调用方不必自己知道这个约定。
            table.set("texture", value.texture_path())?;
            table.set("z", value.z)?;
            table.set("component", value.to_component_str())?;
            Ok(table)
        })?,
    )?;
    lua.globals().set("quad", quad)?;
    // 统一 `transform` 表（WP9）：2D 的 `Transform` 与 3D 的 `Transform3D` 共用同一组读写
    // 函数，按实体**实际拥有**的组件分派。旧行为完全保留：本来有 2D `Transform` 的实体走的
    // 还是原来那条分支（`space` 是新增字段，不影响既有取值），而以前"3D 实体调用 transform
    // 直接报错"的情况现在会落到 3D 分支。
    //
    // 刻意不做的一件事：**3D 下不接受标量 `rotation`**。2D 的 rotation 是绕 Z 的弧度，3D 的
    // rotation 是四元数，把标量悄悄当成绕 Z 旋转属于静默近似，与本项目"不静默改变语义"的
    // 原则冲突，因此拒绝并指向 `transform3d.set`。

    let mesh = lua.create_table()?;
    let s = Bindings::new(world, state, project_root);
    mesh.set(
        "set",
        lua.create_function(move |_, (id, path): (i64, String)| {
            let id = entity_from_lua(id)?;
            let renderer = MeshRenderer::from_component_str(&format!("mesh={path}"))
                .map_err(|error| api_error(error.to_string()))?;
            let state = &s;
            require_alive(state, id)?;
            state
                .world_mut()?
                .insert(id, renderer)
                .map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let s = Bindings::new(world, state, project_root);
    mesh.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let Some(renderer) = world.get::<MeshRenderer>(id).map_err(world_error)? else {
                return Ok(mlua::Value::Nil);
            };
            Ok(mlua::Value::String(lua.create_string(&renderer.mesh)?))
        })?,
    )?;
    let s = Bindings::new(world, state, project_root);
    mesh.set(
        "clear",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            state
                .world_mut()?
                .remove::<MeshRenderer>(id)
                .map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    lua.globals().set("mesh", mesh)?;

    let light = lua.create_table()?;
    let s = Bindings::new(world, state, project_root);
    light.set(
        "set_active",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            if state
                .world()?
                .get::<Light>(id)
                .map_err(world_error)?
                .is_none()
            {
                return Err(api_error("entity has no light"));
            }
            state.set_active_light(id)?;
            Ok(true)
        })?,
    )?;
    let s = Bindings::new(world, state, project_root);
    light.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let Some(light) = world.get::<Light>(id).map_err(world_error)? else {
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
    let s = Bindings::new(world, state, project_root);
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
                let state = &s;
                require_alive(state, id)?;
                if !intensity.is_finite() || intensity < 0.0 {
                    return Err(api_error("light intensity must be finite and non-negative"));
                }
                let values = [dx, dy, dz, r, g, b];
                if values.iter().any(|value| !value.is_finite()) {
                    return Err(api_error("light parameters must be finite"));
                }
                let mut world = state.world_mut()?;
                let light = world
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

    Ok(())
}
