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
use crate::light::{Light, LightKind};
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
        "set_material",
        lua.create_function(move |_, (id, path): (i64, Option<String>)| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let mut renderer = {
                let world = state.world()?;
                world.get::<MeshRenderer>(id).map_err(world_error)?.cloned()
            }
            .ok_or_else(|| api_error("entity has no mesh renderer"))?;
            // `None`/空串清除覆盖，回到 glTF 内嵌材质；路径存在性由渲染器加载时
            // 校验，失败会告警并回退，不会把实体变成不可见。
            renderer.material = path.filter(|value| !value.is_empty());
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
            t.set(
                "kind",
                match light.kind {
                    LightKind::Directional => "directional",
                    LightKind::Point => "point",
                },
            )?;
            t.set("px", light.position.x)?;
            t.set("py", light.position.y)?;
            t.set("pz", light.position.z)?;
            t.set("range", light.range)?;
            Ok(t)
        })?,
    )?;
    // §21.164 点光运行时移动：篝火/火堆跟着燃烧物走，不再钉死在场景坐标。
    // 只允许改 Point 光源的 position；方向光的主光方向仍走 `light.set`。
    let s = Bindings::new(world, state, project_root);
    light.set(
        "move",
        lua.create_function(move |_, (id, x, y, z): (i64, f32, f32, f32)| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            if [x, y, z].iter().any(|value| !value.is_finite()) {
                return Err(api_error("light position must be finite"));
            }
            let mut world = state.world_mut()?;
            let light = world
                .get_mut::<Light>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no light"))?;
            if light.kind != LightKind::Point {
                return Err(api_error("light.move only accepts point lights"));
            }
            light.position = Vec3::new(x, y, z);
            Ok(())
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

    // Built-in procedural terrain sampling. The spec is exactly the
    // `MeshRenderer.mesh` string (`__terrain__?size=..&...`), so gameplay
    // placement and the rendered mesh cannot drift apart.
    let terrain3d = lua.create_table()?;
    terrain3d.set(
        "height_at",
        lua.create_function(move |_, (spec, x, z): (String, f32, f32)| {
            let spec = crate::terrain::TerrainSpec::parse(&spec)
                .map_err(|error| api_error(error.to_string()))?;
            if !x.is_finite() || !z.is_finite() {
                return Err(api_error("terrain3d.height_at coordinates must be finite"));
            }
            Ok(spec.height(x, z))
        })?,
    )?;
    terrain3d.set(
        "bounds",
        lua.create_function(move |lua, spec: String| {
            let spec = crate::terrain::TerrainSpec::parse(&spec)
                .map_err(|error| api_error(error.to_string()))?;
            let (min, max) = spec.bounds();
            let table = lua.create_table()?;
            for (index, value) in min.iter().enumerate() {
                table.set(index + 1, *value)?;
            }
            for (index, value) in max.iter().enumerate() {
                table.set(index + 4, *value)?;
            }
            Ok(table)
        })?,
    )?;
    terrain3d.set(
        "normal_at",
        lua.create_function(move |_, (spec, x, z): (String, f32, f32)| {
            let spec = crate::terrain::TerrainSpec::parse(&spec)
                .map_err(|error| api_error(error.to_string()))?;
            if !x.is_finite() || !z.is_finite() {
                return Err(api_error("terrain3d.normal_at coordinates must be finite"));
            }
            let n = spec.normal(x, z);
            Ok((n[0], n[1], n[2]))
        })?,
    )?;
    terrain3d.set(
        "slope_at",
        lua.create_function(move |_, (spec, x, z): (String, f32, f32)| {
            let spec = crate::terrain::TerrainSpec::parse(&spec)
                .map_err(|error| api_error(error.to_string()))?;
            if !x.is_finite() || !z.is_finite() {
                return Err(api_error("terrain3d.slope_at coordinates must be finite"));
            }
            Ok(spec.slope_degrees(x, z))
        })?,
    )?;
    terrain3d.set(
        "raise",
        lua.create_function(
            move |_, (spec, x, z, radius, delta): (String, f32, f32, f32, f32)| {
                let mut spec = crate::terrain::TerrainSpec::parse(&spec)
                    .map_err(|error| api_error(error.to_string()))?;
                if !x.is_finite() || !z.is_finite() || !radius.is_finite() || !delta.is_finite() {
                    return Err(api_error(
                        "terrain3d.raise coordinates, radius and delta must be finite",
                    ));
                }
                spec.push_edit(crate::terrain::TerrainEdit {
                    x,
                    z,
                    radius,
                    delta,
                })
                .map_err(|error| api_error(error.to_string()))?;
                spec.validate()
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(spec.mesh_path())
            },
        )?,
    )?;
    terrain3d.set(
        "splat_at",
        lua.create_function(move |_, (spec, x, z): (String, f32, f32)| {
            let spec = crate::terrain::TerrainSpec::parse(&spec)
                .map_err(|error| api_error(error.to_string()))?;
            if !x.is_finite() || !z.is_finite() {
                return Err(api_error("terrain3d.splat_at coordinates must be finite"));
            }
            let weights = spec.splat_weights(x, z);
            Ok((weights[0], weights[1], weights[2], weights[3]))
        })?,
    )?;
    terrain3d.set(
        "raycast",
        lua.create_function(
            move |lua,
                  (spec, ox, oy, oz, dx, dy, dz, max): (
                String,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
            )| {
                let spec = crate::terrain::TerrainSpec::parse(&spec)
                    .map_err(|error| api_error(error.to_string()))?;
                if [ox, oy, oz, dx, dy, dz, max]
                    .iter()
                    .any(|value| !value.is_finite())
                {
                    return Err(api_error("terrain3d.raycast arguments must be finite"));
                }
                if max <= 0.0 {
                    return Err(api_error("terrain3d.raycast distance must be positive"));
                }
                let Some(hit) = spec.raycast([ox, oy, oz], [dx, dy, dz], max) else {
                    return Ok(mlua::Value::Nil);
                };
                let table = lua.create_table()?;
                let values = [
                    hit.position[0],
                    hit.position[1],
                    hit.position[2],
                    hit.normal[0],
                    hit.normal[1],
                    hit.normal[2],
                    hit.distance,
                ];
                for (index, value) in values.iter().enumerate() {
                    table.set(index + 1, *value)?;
                }
                Ok(mlua::Value::Table(table))
            },
        )?,
    )?;
    lua.globals().set("terrain3d", terrain3d)?;

    // Built-in procedural water surface. Same spec string as the MeshRenderer.
    let water3d = lua.create_table()?;
    water3d.set(
        "height_at",
        lua.create_function(move |_, (spec, x, z): (String, f32, f32)| {
            let spec = crate::water::WaterSpec::parse(&spec)
                .map_err(|error| api_error(error.to_string()))?;
            if !x.is_finite() || !z.is_finite() {
                return Err(api_error("water3d.height_at coordinates must be finite"));
            }
            Ok(spec.height(x, z))
        })?,
    )?;
    water3d.set(
        "level",
        lua.create_function(move |_, spec: String| {
            let spec = crate::water::WaterSpec::parse(&spec)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(spec.level)
        })?,
    )?;
    water3d.set(
        "depth_at",
        lua.create_function(
            move |_, (water_spec, terrain_spec, x, z): (String, String, f32, f32)| {
                let water = crate::water::WaterSpec::parse(&water_spec)
                    .map_err(|error| api_error(error.to_string()))?;
                let terrain = crate::terrain::TerrainSpec::parse(&terrain_spec)
                    .map_err(|error| api_error(error.to_string()))?;
                if !x.is_finite() || !z.is_finite() {
                    return Err(api_error("water3d.depth_at coordinates must be finite"));
                }
                Ok(water.depth(&terrain, x, z))
            },
        )?,
    )?;
    lua.globals().set("water3d", water3d)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_lua_api::ProjectRoot;

    fn install_render_lua(lua: &Lua) {
        let state = RenderableState::handle();
        let world = Handle::new(World::new(), "world");
        let root = ProjectRoot::handle();
        install(lua, &state, &world, &root).unwrap();
    }

    /// The terrain raycast binding must return the same surface the height
    /// probe sees, and a miss must be `nil` rather than an error.
    #[test]
    fn terrain_raycast_binding_returns_hit_or_nil() {
        let lua = Lua::new();
        install_render_lua(&lua);
        let hit: mlua::Table = lua
            .load(
                r#"return terrain3d.raycast(
                    "__terrain__?size=240&res=96&amp=2.4&seed=7",
                    12.5, 50.0, -8.0, 0.0, -1.0, 0.0, 200.0)"#,
            )
            .eval()
            .unwrap();
        let x: f32 = hit.get(1).unwrap();
        let y: f32 = hit.get(2).unwrap();
        let z: f32 = hit.get(3).unwrap();
        let distance: f32 = hit.get(7).unwrap();
        let expected = crate::terrain::TerrainSpec::default().height(12.5, -8.0);
        assert!((x - 12.5).abs() < 1e-3);
        assert!((z + 8.0).abs() < 1e-3);
        assert!((y - expected).abs() < 1e-3);
        assert!((distance - (50.0 - expected)).abs() < 1e-3);

        let (grass, moss, litter, bare): (f32, f32, f32, f32) = lua
            .load(
                r#"return terrain3d.splat_at(
                    "__terrain__?size=240&res=96&amp=2.4&seed=7", 12.5, -8.0)"#,
            )
            .eval()
            .unwrap();
        assert!((grass + moss + litter + bare - 1.0).abs() < 1e-5);
        assert!(
            [grass, moss, litter, bare]
                .iter()
                .all(|weight| *weight >= 0.0)
        );

        let raised: String = lua
            .load(
                r#"local base = "__terrain__?size=240&res=96&amp=2.4&seed=7"
                local edited = terrain3d.raise(base, 0.0, 0.0, 5.0, 1.5)
                local before = terrain3d.height_at(base, 0.0, 0.0)
                local after = terrain3d.height_at(edited, 0.0, 0.0)
                assert(after - before > 1.4, "edit must raise the center")
                return edited"#,
            )
            .eval()
            .unwrap();
        assert!(raised.contains("&edit=0:0:5:1.5"));

        let miss: mlua::Value = lua
            .load(
                r#"return terrain3d.raycast(
                    "__terrain__?size=240&res=96&amp=2.4&seed=7",
                    0.0, 50.0, 0.0, 0.0, 1.0, 0.0, 50.0)"#,
            )
            .eval()
            .unwrap();
        assert!(matches!(miss, mlua::Value::Nil));
    }
}
