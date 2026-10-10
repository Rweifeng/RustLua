//! `LuaRuntime` 的场景切换：应用 Lua 侧 `scene.load(name)` 挂起的请求。
//!
//! 切换要保住三样东西：`persistent` 实体的快照（按名字，不按句柄）、日进度钩子的先后顺序、
//! 以及切换失败时的诊断落点。

// WP11-USE:BEGIN
use crate::script::LuaRuntime;
use crate::script::PersistedEntity;
use crate::script::call_optional;
use crate::script::error::LuaBridgeError;
use rustlua_ecs::particle::ParticleSystem;
use rustlua_ecs::world::World;
use rustlua_scene::scene::Scene;
use std::path::PathBuf;
// WP11-USE:END

impl LuaRuntime {
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
        // 经 `vfs::read` 取字节（**不是** `Scene::load`）：后者的 `fs::read` 在 wasm 上
        // 读不到任何工程文件——第 45 轮 jhyt webgl 产物上，标题确认触发的切换就死在
        // 这一步（日志只有 `scene file not found` 与一张空白页）。这里与 `Scene::load`
        // 的行为逐字节一致：原始字节 + `from_packed_bytes` 解码，桌面路径仍是文件系统。
        let bytes = rustlua_infra::vfs::read(&path).map_err(|error| {
            LuaBridgeError::Api(format!("scene {name} failed to load: {error}"))
        })?;
        let scene = Scene::from_packed_bytes(&path, &bytes).map_err(|error| {
            LuaBridgeError::Api(format!("scene {name} failed to load: {error}"))
        })?;
        {
            let mut state = self.state.borrow_mut();
            state.world = World::new();
            // docs/60 §4.1：场景生命周期内的清理按领域收敛到各自
            // `clear_for_scene_transition`，新增场景态字段只需登记到所属域。
            state.physics.clear_for_scene_transition();
            state.particles = ParticleSystem::new();
            state.entity_state.clear_for_scene_transition();
            state.ui_state.clear_for_scene_transition();
            state.inventory.clear_for_scene_transition();
            state.schema_defs.clear();
            // M63c：运行期 `prefab.spawn` 的实例登记表与它的布局节点表都是**世界内**状态。
            // 上面刚把 `state.world` 换成新世界，登记表里的实体句柄因此全部失效，留着有两处后果：
            // 旧 handle 再 `prefab.despawn` 会拿着死句柄去新世界上删（标题界面点按钮离开时直接
            // 报错，M20-09c 就是这么红的）；`prefab_ui_nodes` 是按**槽位号**索引的，槽位会被新
            // 世界的无关实体复用（幽灵布局节点：既画不出该画的，又可能顶掉别人的矩形）。
            // 句柄计数器**不**复位：复位会让旧 handle 认到新实例。
            state.prefab_spawns.clear();
            state.prefab_ui_nodes.clear();
            state.scene_pending_load = None;
            state.scene = Some(name.clone());
            state.active_camera = None;
            state.active_light = None;
        }
        let instantiated = self.instantiate_scene(&scene)?;
        // 恢复持久实体：新场景未定义的名字用持久快照重生（新场景定义胜出）。
        {
            let mut state = self.state.borrow_mut();
            let persisted: Vec<PersistedEntity> =
                state.entity_state.persistent.values().cloned().collect();
            for entity in persisted {
                if state.entity_state.names.contains_key(&entity.name) {
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
            reason: error.to_string(),
        })?;
        tracing::info!(target: "scene", name = %name, entities = instantiated, "[scene] switched");
        Ok(())
    }
}
