//! 实体注册域的能力单元（P1 ⑤-13 从 `rustlua-scripting` 搬来）。
//!
//! 名字索引、持久化 id、血量、标签、场景声明表——这些都不是"某个组件"，而是**实体身份**
//! 的运行期账本，所以它跟着 `EntityId` 住进 ECS。脚本层与宿主都只持一个 `Handle<EntityState>`。
//!
//! 两处跨域写按"倒置"处理，ECS 不反向依赖别的领域 crate：
//!
//! - **物理刚体摘除**：`entity.destroy` 只往 [`EntityState::pending_physics_removals`] 里记一笔，
//!   由宿主在推进物理前统一摘（此前是绑定里直接 `physics.world_mut().remove_body`，那是 ECS
//!   绑定反向依赖物理域）。
//! - **交互提示文案**：`EntityState.interact_prompt` 只存文本；把它画成屏幕文字是 UI 域的事
//!   （脚本层的 `interact.prompt` 同时写两边）。

use crate::entity::EntityId;
use crate::spatial_transform::SceneEntityId;
use rustlua_infra::ids::AssetId;
use rustlua_lua_api::Handle;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};

thread_local! {
    /// 观测点：反向索引重建次数（两个 crate 的契约测试都读它：改名才重建、稳态零重建）。
    /// 代价是每次重建一次 thread-local 自增——重建本身就是 O(名字数)，可以忽略。
    static ENTITY_NAME_REBUILDS: Cell<u32> = const { Cell::new(0) };
}

/// 反向索引至今重建了几次。
#[must_use]
pub fn entity_name_rebuilds() -> u32 {
    ENTITY_NAME_REBUILDS.with(|count| count.get())
}

/// 实体名索引：**正向**（名字 → 实体）是唯一真源，**反向**（实体 → 它的名字们）是惰性派生的缓存。
///
/// **为什么要合成一个类型**（`docs/60` §6.1 复核，2026-10-06）：反向查找原本是
/// `by_name.iter().find(...)` 线性扫，而宿主在**实体循环里**调它（`rustlua-scripting` 的
/// `mesh_ui` 每个实体一次）→ 实体数 n 时每帧 O(n²)。
///
/// 只"再加一张反向表"是不够的——两张表各自维护就一定会漂移，而漂移的后果（名字张冠李戴）
/// 比慢一点严重得多。所以两个方向由**这一个类型**一起管：改名的入口只有下面这几个方法，
/// 脏标记在方法里置位，**没有"忘了同步反向表"这种可能**。
///
/// 重建策略与 `TypedStorage::ensure_order` 同构：改动只标脏，下一次反查整体重建一次——
/// 稳态（无改名）每帧零重建。
#[derive(Debug, Default)]
pub struct EntityNames {
    /// 名字 → 实体。BTreeMap 的有序性是 `entity_names_by_slot` 这类**确定性输出**的基础。
    by_name: BTreeMap<String, EntityId>,
    /// 实体 → 它登记过的名字，按 `by_name` 的键序收集：`first_name` 取第一个，与改造前
    /// `iter().find` 的结果**逐字相同**（同一实体挂多个名字时这一点尤其重要）。
    by_entity: RefCell<HashMap<EntityId, Vec<String>>>,
    reverse_dirty: Cell<bool>,
}

impl EntityNames {
    pub fn clear(&mut self) {
        self.by_name.clear();
        self.reverse_dirty.set(true);
    }

    /// 登记一个名字。语义与改造前的 `BTreeMap::insert` **完全一致**：同名会改指到新实体，
    /// 而同一实体拿到新名字时**旧名字仍然指向它**（"名字可冲突、可多个"是本表的既有契约，
    /// 本次只加索引不改语义）。
    pub fn insert(&mut self, name: String, id: EntityId) {
        self.by_name.insert(name, id);
        self.reverse_dirty.set(true);
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<EntityId> {
        self.by_name.get(name).copied()
    }

    #[must_use]
    pub fn contains_key(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// 名字仍指向 `id` 时才摘掉它（prefab 实例销毁用：只摘自己那一份，不误伤别人）。
    pub fn remove_name_if(&mut self, name: &str, id: EntityId) {
        if self.by_name.get(name) == Some(&id) {
            self.by_name.remove(name);
            self.reverse_dirty.set(true);
        }
    }

    /// 摘掉所有指向 `id` 的名字（实体销毁路径）。
    pub fn remove_entity(&mut self, id: EntityId) {
        let before = self.by_name.len();
        self.by_name.retain(|_, value| *value != id);
        if self.by_name.len() != before {
            self.reverse_dirty.set(true);
        }
    }

    /// 按名字有序遍历（`entity_names_by_slot` 等确定性输出的入口）。
    pub fn iter_by_name(&self) -> impl Iterator<Item = (&String, &EntityId)> {
        self.by_name.iter()
    }

    /// 反查缓存的惰性重建。走 `&self` + 内部可变性——外层的借用方式一字不改
    /// （所以这几个 getter 不会给 Lua 重入多开一个可变借用的口子）。
    fn ensure_reverse(&self) {
        if !self.reverse_dirty.get() {
            return;
        }
        ENTITY_NAME_REBUILDS.with(|count| count.set(count.get() + 1));
        let mut built: HashMap<EntityId, Vec<String>> = HashMap::new();
        for (name, id) in &self.by_name {
            built.entry(*id).or_default().push(name.clone());
        }
        *self.by_entity.borrow_mut() = built;
        self.reverse_dirty.set(false);
    }

    /// `entity_name` 的实现：与旧 `iter().find` **同结果**（BTreeMap 键序里的第一个）。
    #[must_use]
    pub fn first_name(&self, id: EntityId) -> Option<String> {
        self.ensure_reverse();
        self.by_entity
            .borrow()
            .get(&id)
            .and_then(|names| names.first().cloned())
    }

    /// 该实体的**任一**名字命中谓词即为真（`is_entity_scene_text_hidden` 用：
    /// 原来是线性扫全表，现在是 O(该实体的名字数)）。
    #[must_use]
    pub fn any_name_matches(&self, id: EntityId, predicate: impl Fn(&str) -> bool) -> bool {
        self.ensure_reverse();
        self.by_entity
            .borrow()
            .get(&id)
            .is_some_and(|names| names.iter().any(|name| predicate(name)))
    }
}

/// 持久化实体的作者态快照（组件 `rustlua_scene` 里的 `Persistent` 或旧 `metadata` 标记）。
/// 以**实体名**为键（切场景后唯一稳定的锚点），由脚本层在场景切换时重建。
#[derive(Clone, Debug, Default)]
pub struct PersistedEntity {
    pub name: String,
    pub components: BTreeMap<String, String>,
    pub resources: BTreeMap<String, AssetId>,
    /// 保留元数据（含旧的 `persistent` 标志）；当前恢复只用 components。
    pub metadata: BTreeMap<String, String>,
    /// 作者态的父实体**名字**：切场景后父实体的句柄会变，名字是唯一稳定锚点。
    pub parent_name: Option<String>,
}

/// `ScriptState` 按领域拆分后的**实体注册域**。
///
/// 场景切换清理走 [`EntityState::clear_for_scene_transition`]——新字段若属于场景
/// 生命周期，加进这里而不是散落的 `state.x.clear()` 行。
#[derive(Debug, Default)]
pub struct EntityState {
    /// 名字索引（正向真源 + 惰性反向缓存，见 [`EntityNames`]）。
    pub names: EntityNames,
    pub health: HashMap<EntityId, f32>,
    /// Stable project-level entity IDs (`persist_id` → live entity). Distinct
    /// from `names` (display/authoring name, collision-prone): a persist ID is
    /// unique across the runtime and is what save/snapshot uses to re-bind
    /// runtime entities (crops, buildings, containers, NPCs) without
    /// duplicating or misbinding them.
    pub persist: HashMap<String, EntityId>,
    /// 跨场景保留的实体快照（名字 → 作者态），场景切换时**刻意不清**。
    pub persistent: BTreeMap<String, PersistedEntity>,
    /// Per-entity string tags used by interact queries and gameplay scripts.
    /// P3-①：纯按键查找、从不按序遍历（有序性只在存档/序列化侧需要），故用 HashMap。
    pub tags: HashMap<EntityId, HashMap<String, String>>,
    /// Optional on-screen interaction prompt text (`interact.prompt`).
    pub interact_prompt: Option<String>,
    /// Authoring ID → live ECS handle. The editor uses this to preview a drag
    /// by mutating one component without rebuilding the whole runtime.
    pub scene_entities: BTreeMap<SceneEntityId, EntityId>,
    /// 待宿主摘除的物理刚体（`entity.destroy` 记账，物理推进前统一摘；ECS 不反向依赖物理域）。
    pub pending_physics_removals: Vec<EntityId>,
}

impl EntityState {
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "entity_state")
    }

    /// `entity.destroy` 的记账口：实体已经删了，物理刚体由宿主下一步摘。
    pub fn queue_physics_removal(&mut self, entity: EntityId) {
        self.pending_physics_removals.push(entity);
    }

    /// 宿主取走待摘刚体（取走即清）。
    pub fn take_physics_removals(&mut self) -> Vec<EntityId> {
        std::mem::take(&mut self.pending_physics_removals)
    }

    /// 场景切换时丢弃**当前场景**的注册数据；`persistent` 刻意保留（跨场景快照）。
    pub fn clear_for_scene_transition(&mut self) {
        self.names.clear();
        self.persist.clear();
        self.health.clear();
        self.tags.clear();
        self.scene_entities.clear();
        self.interact_prompt = None;
        self.pending_physics_removals.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_names_itself_for_reentrancy_errors() {
        let state = EntityState::handle();
        let _guard = state.borrow_mut().unwrap();
        let message = state.borrow().unwrap_err().to_string();
        assert!(message.contains("already borrowed"), "{message}");
        assert!(message.contains("entity_state"), "{message}");
    }

    /// 反向索引：稳态零重建，改名后下一次反查重建一次。
    #[test]
    fn reverse_index_rebuilds_only_after_a_change() {
        let mut state = EntityState::default();
        let first = EntityId::from_raw(1, 1);
        let second = EntityId::from_raw(2, 1);
        state.names.insert("hero".into(), first);
        state.names.insert("npc".into(), second);
        let before = entity_name_rebuilds();
        assert_eq!(state.names.first_name(first).as_deref(), Some("hero"));
        assert_eq!(state.names.first_name(second).as_deref(), Some("npc"));
        assert!(entity_name_rebuilds() > before, "首次反查要建一次索引");
        let settled = entity_name_rebuilds();
        let _ = state.names.first_name(first);
        assert_eq!(entity_name_rebuilds(), settled, "无改名不重建");
        state.names.insert("boss".into(), second);
        assert_eq!(
            state.names.first_name(second).as_deref(),
            Some("boss"),
            "多名字取键序第一个"
        );
        assert!(state.names.any_name_matches(second, |name| name == "boss"));
    }

    /// 销毁摘名字与刚体记账都按预期工作，且切场景清空待摘队列。
    #[test]
    fn destroy_bookkeeping_and_scene_transition() {
        let mut state = EntityState::default();
        let id = EntityId::from_raw(7, 1);
        state.names.insert("crate".into(), id);
        state.health.insert(id, 10.0);
        state.names.remove_entity(id);
        state.health.remove(&id);
        state.queue_physics_removal(id);
        assert_eq!(state.take_physics_removals(), vec![id]);
        assert!(state.take_physics_removals().is_empty(), "取走即清");
        state.persistent.insert(
            "kept".into(),
            PersistedEntity {
                name: "kept".into(),
                ..PersistedEntity::default()
            },
        );
        state.queue_physics_removal(id);
        state.clear_for_scene_transition();
        assert!(state.persistent.contains_key("kept"), "跨场景快照不该被清");
        assert!(state.pending_physics_removals.is_empty());
    }
}
