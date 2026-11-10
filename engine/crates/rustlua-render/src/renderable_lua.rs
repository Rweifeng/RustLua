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
use crate::instance_batch::{
    BatchMotion3D, ForwardAxis, Instance3D, InstanceBatch3D, InstanceMotion3D, MotionKind,
    instances_from_packed,
};
use crate::light::{Light, LightKind};
use crate::mesh3d::MeshRenderer;
use crate::sprite::{SpriteAsset, SpriteComponent};
use mlua::{Lua, Result as LuaResult};
use rustlua_animation::animation::AnimationStateMachine;
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::{World, WorldError};
use rustlua_lua_api::{Handle, ProjectRoot, api_error};
use rustlua_math::math3d::Vec3;
use std::cell::{Cell, Ref, RefMut};

/// 渲染域里"跨帧的脚本可见状态"：本帧主光。
///
/// 与相机 `set_active` 同构：脚本只**登记选择**，渲染各 pass 决定怎么用它；场景切换让旧
/// 实体 id 失效（换图后旧句柄不该还点着上一张图的灯）。
#[derive(Debug, Default)]
pub struct RenderableState {
    active_light: Option<EntityId>,
    /// 场景时钟（秒）：宿主每帧推进。程序化实例运动（`InstanceBatch3D` 的路径流）按它求值，
    /// 于是"引擎侧画出来的姿态"与"脚本侧读回的姿态"看到的是同一个时刻。
    scene_time: Cell<f32>,
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

    /// 场景时钟（秒）。
    #[must_use]
    pub fn scene_time(&self) -> f32 {
        self.scene_time.get()
    }

    /// 推进场景时钟（秒）。
    pub fn set_scene_time(&self, seconds: f32) {
        self.scene_time.set(seconds);
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

/// Lua 表 → 一个实例：`x/y/z` 位置、`sx/sy/sz` 缩放、姿态二选一——
/// `yaw`（绕 +Y 的弧度）或 `q = {x,y,z,w}`（四元数）。
///
/// 非有限值一律报错而不是静默当成 0：位置写成 NaN 的蚂蚁会在剔除阶段变成"看不见"，
/// 而"看不见"与"没画"在画面上分不开——那种失败必须停在调用点。
fn instance_from_table(table: &mlua::Table, has_motion: bool) -> LuaResult<Instance3D> {
    fn finite(key: &str, value: f32) -> LuaResult<f32> {
        if value.is_finite() {
            Ok(value)
        } else {
            Err(api_error(format!("`{key}` must be a finite number")))
        }
    }
    let number = |key: &str| -> LuaResult<Option<f32>> {
        match table.get::<Option<f32>>(key)? {
            Some(value) => finite(key, value).map(Some),
            None => Ok(None),
        }
    };
    // 批次带运动时位置由路径决定：实例表里再写 x/y/z 就是**两份真相**，直接报错而不是
    // 悄悄丢掉一份（丢掉的那份会让"我明明设了位置"变成一句无法解释的假话）。
    if has_motion {
        for key in ["x", "y", "z"] {
            if table.get::<Option<f32>>(key)?.is_some() {
                return Err(api_error(format!(
                    "instance table must not carry `{key}` when the batch has motion (the position comes from the path)"
                )));
            }
        }
    }
    let position = [
        number("x")?.unwrap_or(0.0),
        number("y")?.unwrap_or(0.0),
        number("z")?.unwrap_or(0.0),
    ];
    let scale = [
        number("sx")?.unwrap_or(1.0),
        number("sy")?.unwrap_or(1.0),
        number("sz")?.unwrap_or(1.0),
    ];
    let rotation = match table.get::<Option<mlua::Table>>("q")? {
        Some(quaternion) => {
            let component = |key: &str| -> LuaResult<f32> {
                match quaternion.get::<Option<f32>>(key)? {
                    Some(value) => finite(key, value),
                    None => Ok(0.0),
                }
            };
            [
                component("x")?,
                component("y")?,
                component("z")?,
                // w 缺省是 1（单位四元数），不是 0（那会把整个实例压成零向量）。
                match quaternion.get::<Option<f32>>("w")? {
                    Some(value) => finite("w", value)?,
                    None => 1.0,
                },
            ]
        }
        None => {
            let yaw = number("yaw")?.unwrap_or(0.0);
            let half = yaw * 0.5;
            [0.0, half.sin(), 0.0, half.cos()]
        }
    };
    let motion = InstanceMotion3D {
        offset: number("s")?.unwrap_or(0.0),
        speed: number("speed")?.unwrap_or(1.0),
        align: table.get::<Option<bool>>("align")?.unwrap_or(false),
        // 前向轴是**模型自己的约定**，引擎不猜：`"x"`（默认，模型 +X 指向行进方向）或
        // `"z"`（模型 +Z 指向行进方向）。写错的名字直接报错，不静默落回默认。
        forward: match table.get::<Option<String>>("forward")?.as_deref() {
            None | Some("x") => ForwardAxis::X,
            Some("z") => ForwardAxis::Z,
            Some(other) => {
                return Err(api_error(format!(
                    "instance `forward` must be \"x\" or \"z\" (got `{other}`)"
                )));
            }
        },
    };
    Ok(Instance3D {
        position,
        rotation,
        scale,
        // 没有运动的批次不带运动参数（静态路径逐位不变）。
        motion: has_motion.then_some(motion),
    })
}

/// Lua 值 → 实例表：接受**表**（逐实例 `{x,y,z,...}`）或**打包字符串**
/// （每实例 28 字节：`x,y,z,sx,sy,sz,yaw` 小端 f32）。
///
/// `what` 是调用点名（`mesh.set_batch` / `mesh.set_batch_range`），错误串按它前缀，
/// 两个入口的报错形状因此逐字一致、只差入口名。
fn instances_from_lua(
    value: mlua::Value,
    has_motion: bool,
    what: &str,
) -> LuaResult<Vec<Instance3D>> {
    match value {
        mlua::Value::Table(table) => {
            let mut parsed = Vec::with_capacity(table.raw_len());
            for (index, item) in table.sequence_values::<mlua::Table>().enumerate() {
                let item =
                    item.map_err(|error| api_error(format!("{what} instance #{index}: {error}")))?;
                let instance = instance_from_table(&item, has_motion)
                    .map_err(|error| api_error(format!("{what} instance #{index}: {error}")))?;
                parsed.push(instance);
            }
            Ok(parsed)
        }
        mlua::Value::String(blob) => {
            // 打包口径自带位置：再给运动表就是两份真相，直接报错。
            if has_motion {
                return Err(api_error(format!(
                    "{what}: packed instances carry their own positions; drop the motion table"
                )));
            }
            instances_from_packed(&blob.as_bytes())
                .map_err(|error| api_error(format!("{what} packed: {error}")))
        }
        other => Err(api_error(format!(
            "{what}: instances must be a table or a packed string (got {})",
            other.type_name()
        ))),
    }
}

/// Lua 表 → 批次级程序化运动。
///
/// `path` 是折线点表（每点 `{x,y,z}`，三个数都必须给且有限），`closed` 默认假，
/// `speed` 默认 1（世界单位/秒）。`kind` 目前只认 `"path"`。
fn motion_from_table(table: &mlua::Table) -> LuaResult<BatchMotion3D> {
    let kind = match table.get::<Option<String>>("kind")? {
        Some(kind) if kind == "path" => MotionKind::Path,
        Some(kind) => {
            return Err(api_error(format!(
                "unknown motion kind `{kind}` (only `path` is supported)"
            )));
        }
        None => MotionKind::Path,
    };
    let points: Vec<mlua::Table> = table.get::<Vec<mlua::Table>>("path")?;
    if points.len() < 2 {
        return Err(api_error("motion `path` needs at least two points"));
    }
    let mut path = Vec::with_capacity(points.len());
    for (index, point) in points.iter().enumerate() {
        let coordinate = |key: &str| -> LuaResult<f32> {
            match point.get::<Option<f32>>(key)? {
                Some(value) if value.is_finite() => Ok(value),
                Some(_) => Err(api_error(format!(
                    "motion path point #{index}: `{key}` must be a finite number"
                ))),
                None => Err(api_error(format!(
                    "motion path point #{index}: `{key}` is missing"
                ))),
            }
        };
        path.push([coordinate("x")?, coordinate("y")?, coordinate("z")?]);
    }
    let closed = table.get::<Option<bool>>("closed")?.unwrap_or(false);
    let speed = match table.get::<Option<f32>>("speed")? {
        Some(value) if value.is_finite() => value,
        Some(_) => return Err(api_error("motion `speed` must be a finite number")),
        None => 1.0,
    };
    Ok(BatchMotion3D {
        kind,
        path,
        closed,
        speed,
    })
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
    Ok(())
}

/// M81：3D 包的四张表（mesh / light / terrain3d / water3d）。领域 crate 不带 feature，
/// 由组合根 `register_all` 在 `3d` 开启时才调用（2D 构建的 Lua 面没有这四张表）。
pub fn install_3d(
    lua: &Lua,
    state: &Handle<RenderableState>,
    world: &Handle<World>,
    project_root: &Handle<ProjectRoot>,
) -> LuaResult<()> {
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
    // P3-R237：实例批次（crowd）。`set_batch` 把 N 份实例变换收进**一个**组件，
    // 渲染侧一次实例化绘制——提取侧也只有**一个**实体（逐个建实体是"N 万只蚂蚁"的
    // 组件与提取开销来源）。
    let s = Bindings::new(world, state, project_root);
    mesh.set(
        "set_batch",
        lua.create_function(
            move |_,
                  (id, path, instances, material, motion): (
                i64,
                String,
                mlua::Value,
                Option<String>,
                Option<mlua::Table>,
            )| {
                let id = entity_from_lua(id)?;
                let state = &s;
                require_alive(state, id)?;
                // P3-R238：运动是可选的第五个参数。给了 motion ⇒ 位置由路径决定，
                // 实例表里不许再写 x/y/z（两份真相直接报错）。
                let motion = motion
                    .as_ref()
                    .map(motion_from_table)
                    .transpose()
                    .map_err(|error| api_error(format!("mesh.set_batch motion: {error}")))?;
                // P3-R239：`instances` 接受**表**（逐实例 `{x,y,z,...}`）或**打包字符串**
                // （每实例 28 字节：x,y,z,sx,sy,sz,yaw 小端 f32）。打包口径免掉"每只建一张表
                // 再逐键 marshal"——那是 Myrmex 实测 3.0 µs/只 的来源（§21.517）。
                let parsed = instances_from_lua(instances, motion.is_some(), "mesh.set_batch")?;
                let batch = InstanceBatch3D {
                    mesh: path,
                    // 空串按"清除覆盖"处理，与 `mesh.set_material` 同一条口径。
                    material: material.filter(|value| !value.is_empty()),
                    instances: parsed,
                    motion,
                };
                state.world_mut()?.insert(id, batch).map_err(world_error)?;
                Ok(true)
            },
        )?,
    )?;
    // P3-R240：局部改段。`set_batch` 要重建整批 N 份实例，改第 k 只也要付 N 份的解析与
    // 分配；`set_batch_range` 只解析被改的那 K 份并就地 splice，数量与顺序都不变。
    //
    // 语义上有三条硬边界（都报错，不静默）：越界段、带运动批次、没有批次的实体。带运动
    // 批次从路径拿位置，改实例表不会改变画出来的姿态——静默成功会成为一句无法解释的谎话。
    let s = Bindings::new(world, state, project_root);
    mesh.set(
        "set_batch_range",
        lua.create_function(move |_, (id, start, instances): (i64, i64, mlua::Value)| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            if start < 0 {
                return Err(api_error(format!(
                    "mesh.set_batch_range: start must be >= 0 (got {start})"
                )));
            }
            let parsed = instances_from_lua(instances, false, "mesh.set_batch_range")?;
            let count = parsed.len();
            let mut world = state.world_mut()?;
            let batch = world
                .get_mut::<InstanceBatch3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("mesh.set_batch_range: entity has no instance batch"))?;
            batch
                .replace_range(start as usize, parsed)
                .map_err(|error| api_error(format!("mesh.set_batch_range: {error}")))?;
            Ok(count)
        })?,
    )?;
    let s = Bindings::new(world, state, project_root);
    mesh.set(
        "clear_batch",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            state
                .world_mut()?
                .remove::<InstanceBatch3D>(id)
                .map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let s = Bindings::new(world, state, project_root);
    mesh.set(
        "batch_count",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let count = world
                .get::<InstanceBatch3D>(id)
                .map_err(world_error)?
                .map_or(0, |batch| batch.instances.len());
            Ok(count)
        })?,
    )?;
    // P3-R238：读回某个实例**当前时刻**的位姿（位置 + yaw）。运动是引擎侧求值的，
    // 脚本要确认"蚂蚁真的在动"就得能读回同一份求值结果——同一个场景时钟、同一套规则。
    let s = Bindings::new(world, state, project_root);
    mesh.set(
        "batch_pose",
        lua.create_function(move |lua, (id, index): (i64, i64)| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            if index < 0 {
                return Err(api_error("mesh.batch_pose: index must be >= 0"));
            }
            let world = state.world()?;
            let batch = world
                .get::<InstanceBatch3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("mesh.batch_pose: entity has no instance batch"))?;
            let time = s
                .state
                .borrow()
                .map(|state| state.scene_time())
                .unwrap_or(0.0);
            let matrix = batch
                .instance_matrix_at(index as usize, time)
                .ok_or_else(|| {
                    api_error(format!(
                        "mesh.batch_pose: instance #{index} has no valid transform"
                    ))
                })?;
            let origin = matrix
                .transform_point(Vec3::new(0.0, 0.0, 0.0))
                .unwrap_or(Vec3::new(0.0, 0.0, 0.0));
            let yaw = batch.instance_yaw_at(index as usize, time).unwrap_or(0.0);
            let pose = lua.create_table()?;
            pose.set("x", origin.x)?;
            pose.set("y", origin.y)?;
            pose.set("z", origin.z)?;
            pose.set("yaw", yaw)?;
            Ok(pose)
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

    // P3-R213：`__cells__` 逐格巢体的**规格面**。与 `terrain3d` 同形：Lua 只经手规格串，
    // 几何由渲染器在网格缓存 miss 时生成 ⇒ 巢体每次挖/填都是「新路径 → 新几何」，不留第二份
    // 隐藏状态，也没有运行期改顶点的入口（引擎里本来就没有这种东西）。
    // 下标一律是 `CellMeshSpec::index` 的扁平下标（0 基，`x + w*(z + d*y)`）；越界直接报错而
    // 不是夹取：夹一下会把「挖错格」变成「挖了隔壁那格」。
    let cells3d = lua.create_table()?;
    cells3d.set(
        "build",
        lua.create_function(move |_, opts: mlua::Table| {
            let number = |key: &str| -> Result<f32, mlua::Error> {
                opts.get::<f32>(key)
                    .map_err(|_| api_error(format!("cells3d.build needs a numeric `{key}`")))
            };
            let cell = number("cell")?;
            let origin = [number("x")?, number("y")?, number("z")?];
            let axes = [number("w")?, number("h")?, number("d")?];
            for axis in axes {
                if !axis.is_finite() || axis.fract() != 0.0 || axis < 1.0 {
                    return Err(api_error(
                        "cells3d.build needs positive integer cell counts for w/h/d",
                    ));
                }
            }
            let mut spec = crate::cellmesh::CellMeshSpec::new(
                cell,
                origin,
                [axes[0] as u32, axes[1] as u32, axes[2] as u32],
            );
            spec.double_sided = opts.get::<bool>("double_sided").unwrap_or(false);
            // `solids` 是实心格下标的数组（1 基数组位、0 基下标值）；省略 = 全空区域。
            if let mlua::Value::Table(list) = opts
                .get::<mlua::Value>("solids")
                .unwrap_or(mlua::Value::Nil)
            {
                for slot in 1..=list.raw_len() {
                    let value: f64 = list.get(slot)?;
                    if !value.is_finite() || value.fract() != 0.0 || value < 0.0 {
                        return Err(api_error(
                            "cells3d.build solids must be non-negative integer cell indices",
                        ));
                    }
                    spec.set_cell(value as usize, true)
                        .map_err(|error| api_error(error.to_string()))?;
                }
            }
            spec.validate()
                .map_err(|error| api_error(error.to_string()))?;
            Ok(spec.mesh_path())
        })?,
    )?;
    // dig/fill 只差写入的位值，但**刻意各写一遍** `create_function`：文档桩生成器按注册点的
    // `create_function` 形状推断签名，抽成公共闭包会让这两个键变成 unmatched（实测 unmatched=2）。
    cells3d.set(
        "dig",
        lua.create_function(move |_, (path, index): (String, f64)| {
            let mut spec = crate::cellmesh::CellMeshSpec::parse(&path)
                .map_err(|error| api_error(error.to_string()))?;
            if !index.is_finite() || index.fract() != 0.0 || index < 0.0 {
                return Err(api_error(
                    "cells3d.dig needs a non-negative integer cell index",
                ));
            }
            spec.set_cell(index as usize, false)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(spec.mesh_path())
        })?,
    )?;
    cells3d.set(
        "fill",
        lua.create_function(move |_, (path, index): (String, f64)| {
            let mut spec = crate::cellmesh::CellMeshSpec::parse(&path)
                .map_err(|error| api_error(error.to_string()))?;
            if !index.is_finite() || index.fract() != 0.0 || index < 0.0 {
                return Err(api_error(
                    "cells3d.fill needs a non-negative integer cell index",
                ));
            }
            spec.set_cell(index as usize, true)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(spec.mesh_path())
        })?,
    )?;
    cells3d.set(
        "count",
        lua.create_function(move |_, path: String| {
            let spec = crate::cellmesh::CellMeshSpec::parse(&path)
                .map_err(|error| api_error(error.to_string()))?;
            Ok((spec.solid_count(), spec.face_count()))
        })?,
    )?;
    cells3d.set(
        "bounds",
        lua.create_function(move |_, path: String| {
            let spec = crate::cellmesh::CellMeshSpec::parse(&path)
                .map_err(|error| api_error(error.to_string()))?;
            let (min, max) = spec.bounds();
            Ok((min[0], min[1], min[2], max[0], max[1], max[2]))
        })?,
    )?;
    cells3d.set(
        "solid_at",
        lua.create_function(move |_, (path, x, y, z): (String, f32, f32, f32)| {
            let spec = crate::cellmesh::CellMeshSpec::parse(&path)
                .map_err(|error| api_error(error.to_string()))?;
            if [x, y, z]
                .iter()
                .any(|value| !value.is_finite() || value.fract() != 0.0 || *value < 0.0)
            {
                return Err(api_error(
                    "cells3d.solid_at needs non-negative integer cell coordinates",
                ));
            }
            Ok(spec.solid_at(x as u32, y as u32, z as u32))
        })?,
    )?;
    cells3d.set(
        "index",
        lua.create_function(move |_, (path, x, y, z): (String, f32, f32, f32)| {
            let spec = crate::cellmesh::CellMeshSpec::parse(&path)
                .map_err(|error| api_error(error.to_string()))?;
            if [x, y, z]
                .iter()
                .any(|value| !value.is_finite() || value.fract() != 0.0 || *value < 0.0)
            {
                return Err(api_error(
                    "cells3d.index needs non-negative integer cell coordinates",
                ));
            }
            // 区域外给 nil 而不是报错：沿边界扫邻居是常态调用。
            Ok(spec
                .index(x as u32, y as u32, z as u32)
                .map(|index| index as f64))
        })?,
    )?;
    lua.globals().set("cells3d", cells3d)?;

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
        // 领域 crate 无 feature：测试永远装全量面（门控只发生在组合根调用点）。
        install_3d(lua, &state, &world, &root).unwrap();
    }

    /// P3-R213：规格面过一遍 Lua —— 3×3×3 满块 build → dig 中心格 → 腔内多出 6 面，
    /// fill 必须把路径**逐字节还原**（规范化 round-trip），越界下标要报错不能夹取。
    /// `index`/`solid_at`/`bounds` 与 Rust 侧同式，区域外给 nil 而不是报错。
    #[test]
    fn cells3d_binding_builds_digs_and_restores() {
        let lua = Lua::new();
        install_render_lua(&lua);
        let path: String = lua
            .load(
                r#"local solids = {}
                for index = 0, 26 do solids[#solids + 1] = index end
                return cells3d.build({ cell = 1, x = 0, y = 0, z = 0, w = 3, h = 3, d = 3, solids = solids })"#,
            )
            .eval()
            .unwrap();
        let (solid, faces): (usize, usize) = lua
            .load(format!("return cells3d.count([[{}]])", path))
            .eval()
            .unwrap();
        assert_eq!((solid, faces), (27, 54), "满块只算表面积，不算 27×6 面");
        let dug: String = lua
            .load(format!("return cells3d.dig([[{}]], 13)", path))
            .eval()
            .unwrap();
        let (solid, faces): (usize, usize) = lua
            .load(format!("return cells3d.count([[{}]])", dug))
            .eval()
            .unwrap();
        assert_eq!(
            (solid, faces),
            (26, 60),
            "挖开中心格：少一格实心、腔内露出 6 面"
        );
        let filled: String = lua
            .load(format!("return cells3d.fill([[{}]], 13)", dug))
            .eval()
            .unwrap();
        assert_eq!(filled, path, "fill 必须原样还原成同一份规范化路径");
        assert_ne!(dug, path, "dig 之后是另一条路径（渲染器据此重生成几何）");
        let center: bool = lua
            .load(format!("return cells3d.solid_at([[{}]], 1, 1, 1)", dug))
            .eval()
            .unwrap();
        assert!(!center, "dug 规格读回中心格必须是空");
        let index: f64 = lua
            .load(format!("return cells3d.index([[{}]], 1, 1, 1)", path))
            .eval()
            .unwrap();
        assert_eq!(index, 13.0, "扁平下标与 Rust 侧 x + w*(z + d*y) 同式");
        let outside: mlua::Value = lua
            .load(format!("return cells3d.index([[{}]], 3, 0, 0)", path))
            .eval()
            .unwrap();
        assert!(matches!(outside, mlua::Value::Nil), "区域外给 nil");
        let bounds: (f32, f32, f32, f32, f32, f32) = lua
            .load(format!("return cells3d.bounds([[{}]])", dug))
            .eval()
            .unwrap();
        assert_eq!(
            bounds,
            (0.0, 0.0, 0.0, 3.0, 3.0, 3.0),
            "包围盒按整区，不随挖空抖动"
        );
        for bad_index in ["27", "-1", "1.5"] {
            let script = format!("return cells3d.dig([[{}]], {})", path, bad_index);
            assert!(
                lua.load(&script).eval::<String>().is_err(),
                "下标 {} 必须报错，不能夹取成别的格",
                bad_index
            );
        }
        let empty: String = lua
            .load(r#"return cells3d.build({ cell = 1, x = 0, y = 0, z = 0, w = 2, h = 2, d = 2 })"#)
            .eval()
            .unwrap();
        let (solid, faces): (usize, usize) = lua
            .load(format!("return cells3d.count([[{}]])", empty))
            .eval()
            .unwrap();
        assert_eq!((solid, faces), (0, 0), "不给 solids = 全空区域");
        assert!(
            lua.load(
                r#"return cells3d.build({ cell = 1, x = 0, y = 0, z = 0, w = 2.5, h = 2, d = 2 })"#
            )
            .eval::<String>()
            .is_err(),
            "格数不许是小数：截断会悄悄少画一层"
        );
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

    /// P3-R238：实例表的前向轴只有 `x`/`z` 两种，写错要报错；带运动的批次里位置由
    /// 路径决定，实例表再写 `x/y/z` 就是两份真相，直接拒。
    #[test]
    fn instance_table_parses_forward_axis_and_rejects_positions_under_motion() {
        let lua = Lua::new();
        let table = |src: &str| -> mlua::Table { lua.load(src).eval().expect("构造实例表") };

        let plain = table("return { s = 1.0, align = true }");
        let instance = instance_from_table(&plain, true).expect("带运动的实例表");
        assert_eq!(
            instance.motion.expect("运动参数").forward,
            ForwardAxis::X,
            "默认必须是 +X 前向（保持既有行为）"
        );

        let z_forward = table(r#"return { s = 1.0, align = true, forward = "z" }"#);
        let instance = instance_from_table(&z_forward, true).expect("+Z 前向");
        assert_eq!(
            instance.motion.expect("运动参数").forward,
            ForwardAxis::Z,
            "+Z 前向要被认出来"
        );

        let bad = table(r#"return { forward = "y" }"#);
        assert!(
            instance_from_table(&bad, true).is_err(),
            "前向轴写错必须被拒（不静默落回默认）"
        );

        let with_position = table("return { x = 1.0 }");
        assert!(
            instance_from_table(&with_position, true).is_err(),
            "带运动时实例表里的位置必须被拒（位置由路径决定）"
        );
    }
}
