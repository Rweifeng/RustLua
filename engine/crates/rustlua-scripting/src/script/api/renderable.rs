//! 可渲染对象：`sprite` / `quad` / `mesh` / `light` / `world_text`。
//! 它们只写组件，实际绘制在渲染层读取这些组件。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::error::{api_error, world_error};
use crate::script::values::{entity_from_lua, require_alive};
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_animation::animation::AnimationStateMachine;
use rustlua_math::math3d::Vec3;
use rustlua_render::light::Light;
use rustlua_render::mesh3d::MeshRenderer;
use rustlua_render::sprite::SpriteAsset;
use rustlua_render::sprite::SpriteComponent;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
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
            let mut state = s.lua_borrow_mut()?;
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
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let value = state
                .world
                .get::<rustlua_render::sprite::Sprite>(id)
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
    // docs/54 §4.3：一等 Quad 的运行时入口。与 `sprite` 分开是因为两者语义不同（Quad 没有纹理
    // 区域、不参与帧动画），而不是同一个东西的两种写法——脚本要"一个纯色矩形"时不该被迫写
    // `texture=__white__` 这种内部哨兵。

    let quad = lua.create_table()?;
    let s = Rc::clone(&state);
    quad.set(
        "set",
        lua.create_function(move |_, (id, component): (i64, String)| {
            let id = entity_from_lua(id)?;
            let parsed = rustlua_render::quad::Quad::from_component_str(&component)
                .map_err(|error| api_error(error.to_string()))?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            state.world.insert(id, parsed).map_err(world_error)?;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    quad.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let value = state
                .world
                .get::<rustlua_render::quad::Quad>(id)
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
    let s = Rc::clone(&state);
    mesh.set(
        "set",
        lua.create_function(move |_, (id, path): (i64, String)| {
            let id = entity_from_lua(id)?;
            let renderer = MeshRenderer::from_component_str(&format!("mesh={path}"))
                .map_err(|error| api_error(error.to_string()))?;
            let mut state = s.lua_borrow_mut()?;
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
            let state = s.lua_borrow()?;
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
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            state
                .world
                .remove::<MeshRenderer>(id)
                .map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    lua.globals().set("mesh", mesh)?;

    let light = lua.create_table()?;
    let s = Rc::clone(&state);
    light.set(
        "set_active",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
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
            let state = s.lua_borrow()?;
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
                let mut state = s.lua_borrow_mut()?;
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

    let world_text = lua.create_table()?;
    let s = Rc::clone(&state);
    world_text.set(
        "attach",
        lua.create_function(move |_, (id, component): (i64, String)| {
            let text = rustlua_scene::world_text::WorldText::from_component_str(&component)
                .map_err(|error| api_error(error.to_string()))?;
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            state.world.insert(id, text).map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    world_text.set(
        "detach",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            let removed = state
                .world
                .remove::<rustlua_scene::world_text::WorldText>(id)
                .map_err(world_error)?
                .is_some();
            Ok(removed)
        })?,
    )?;
    lua.globals().set("world_text", world_text)?;
    Ok(())
}
