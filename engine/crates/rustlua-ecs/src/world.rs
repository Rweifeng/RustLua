//! Entity world and type-erased component storage.
//!
//! WP3 裁定：组件存储**整体**换成 dense（[`crate::dense_storage::DenseStorage`]）。对外契约逐条不变——
//! `for_each*` 仍是 `EntityId` 有序、`entities_with` 仍返回有序 `Vec`、`get`/`insert`/`remove`
//! 语义与原 `HashMap` 实现一致；变化只在内部：查询不再双层哈希（`TypeId` 表 + 实体表）、
//! 稳态帧遍历零分配、删除不再是排序 `Vec` 的 O(n) memmove。

use std::{
    any::{Any, TypeId},
    cell::{Cell, RefCell},
    collections::HashMap,
    fmt,
};

use crate::dense_storage::DenseStorage;
use crate::entity::{EntityAllocator, EntityId};

/// Errors returned by world operations that require a live entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum WorldError {
    /// The entity handle is stale or was never allocated by this world.
    #[error("invalid entity handle: {0}")]
    InvalidEntity(EntityId),
}

trait ComponentStorage: Any {
    fn remove_entity(&mut self, entity: EntityId);
    fn contains_entity(&self, entity: EntityId) -> bool;
    fn entity_ids(&self) -> Vec<EntityId>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

// 测试专用：有序索引的重排次数。`TypedStorage` 的契约是「稳态帧零重排」，而重排产出的 `order`
// 与重排前**逐字节相同**（同一批 id、同一个升序结果）——从外部观察不到，只能数次数。
// 用 `thread_local` 而不是全局计数：`cargo test` 并行跑用例，全局计数会互相污染。
#[cfg(test)]
thread_local! {
    static ORDER_REBUILDS: Cell<u32> = const { Cell::new(0) };
}

/// 本线程累计的有序索引重排次数（测试用）。
#[cfg(test)]
fn order_rebuilds() -> u32 {
    ORDER_REBUILDS.with(|count| count.get())
}

/// dense 组件存储 + **惰性重建的有序 id 索引**。
///
/// 两条契约同时满足：`EntityId` 有序遍历（既有 `for_each*` 契约，见
/// `queries_component_combinations_in_sorted_entity_order`）与 O(1) 增删查。
///
/// `order` 是 dense 当前 id 的升序副本：插入/删除只**标脏**，下一次有序访问时整体重排一次
/// （O(n log n)）。稳态帧（无增删）因此零分配、零哈希；这与"插入时二分插入"相比代价低得多
/// （后者每次插入 O(n) memmove）。
///
/// 两条借用路径（`&self` 的 `ensure_order` 与 `&mut self` 的 `ensure_order_mut`）**共用这一条
/// 契约：先看脏标记**。这条守卫曾经只有 `&self` 那一半有，`for_each_mut` 因此每帧无条件重排——
/// 「稳态零重排」的回归用例是 `for_each_mut_rebuilds_order_only_when_dirty`。
struct TypedStorage<T> {
    dense: DenseStorage<T>,
    order: RefCell<Vec<EntityId>>,
    order_dirty: Cell<bool>,
}

impl<T> TypedStorage<T> {
    const fn new() -> Self {
        Self {
            dense: DenseStorage::new(),
            order: RefCell::new(Vec::new()),
            order_dirty: Cell::new(false),
        }
    }

    /// `&mut self` 路径的重建（不做运行时借用检查：`for_each_mut` 要把 `order` **移出**再遍历，
    /// 不能跨着可变遍历持有 `RefCell` 借用）。
    ///
    /// 与 `ensure_order` **共用同一条契约：先看脏标记**。守卫曾经只有 `&self` 那一半有，于是
    /// `for_each_mut` 在稳态帧每帧做一次 O(n log n) 排序——正好违反 `order` 字段注释写下的
    /// 「稳态帧零分配」。回归用例：`for_each_mut_rebuilds_order_only_when_dirty`。
    fn ensure_order_mut(&mut self) {
        if !self.order_dirty.get() {
            return;
        }
        #[cfg(test)]
        ORDER_REBUILDS.with(|count| count.set(count.get() + 1));
        let order = self.order.get_mut();
        order.clear();
        order.extend_from_slice(self.dense.entities());
        order.sort_unstable();
        self.order_dirty.set(false);
    }

    /// `&self` 路径的重建（`for_each` 只拿得到 `&World`）。
    fn ensure_order(&self) {
        if !self.order_dirty.get() {
            return;
        }
        let mut order = self.order.borrow_mut();
        order.clear();
        order.extend_from_slice(self.dense.entities());
        order.sort_unstable();
        self.order_dirty.set(false);
    }
}

impl<T: 'static> ComponentStorage for TypedStorage<T> {
    fn remove_entity(&mut self, entity: EntityId) {
        if self.dense.remove(entity).is_some() {
            self.order_dirty.set(true);
        }
    }

    fn contains_entity(&self, entity: EntityId) -> bool {
        self.dense.contains(entity)
    }

    fn entity_ids(&self) -> Vec<EntityId> {
        self.ensure_order();
        self.order.borrow().clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Owns entity lifetimes and the component stores attached to them.
#[derive(Default)]
pub struct World {
    entities: EntityAllocator,
    components: HashMap<TypeId, Box<dyn ComponentStorage>>,
    /// P3-⑤：实体 → 其持有组件类型的反向索引（`insert`/`remove`/`despawn` 三个入口
    /// 维护）。`despawn` 借此只碰相关 storage，不再遍历全部已注册组件类型。
    entity_components: HashMap<EntityId, Vec<TypeId>>,
    /// P0-②：组件结构修订号——每次 `insert` / `remove` / `despawn` 递增。缓存方
    /// （如每实体脚本请求表）以它为失效键，无需每帧全量重扫即可确认"没变化"。
    revision: u64,
}

impl fmt::Debug for World {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("World")
            .field("entity_count", &self.entities.len())
            .field("component_type_count", &self.components.len())
            .finish()
    }
}

impl World {
    /// Creates an empty world.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new entity without components.
    pub fn spawn(&mut self) -> EntityId {
        self.entities.spawn()
    }

    /// Destroys an entity and removes all components attached to it.
    pub fn despawn(&mut self, entity: EntityId) -> Result<(), WorldError> {
        if !self.entities.despawn(entity) {
            return Err(WorldError::InvalidEntity(entity));
        }
        self.revision = self.revision.wrapping_add(1);
        if let Some(types) = self.entity_components.remove(&entity) {
            for type_id in types {
                if let Some(storage) = self.components.get_mut(&type_id) {
                    storage.remove_entity(entity);
                }
            }
        }
        Ok(())
    }

    /// 当前组件结构修订号（P0-②：缓存失效判据，只增不减）。
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns whether an entity handle is currently alive in this world.
    #[must_use]
    pub fn is_alive(&self, entity: EntityId) -> bool {
        self.entities.is_alive(entity)
    }

    /// Returns the number of live entities.
    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Inserts or replaces a component and returns the previous value, if any.
    pub fn insert<T: 'static>(
        &mut self,
        entity: EntityId,
        component: T,
    ) -> Result<Option<T>, WorldError> {
        self.ensure_alive(entity)?;
        let storage = self
            .components
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(TypedStorage::<T>::new()));
        let typed = storage
            .as_any_mut()
            .downcast_mut::<TypedStorage<T>>()
            .expect("component storage type id must match its concrete storage");
        let previous = typed.dense.insert(entity, component);
        self.revision = self.revision.wrapping_add(1);
        if previous.is_none() {
            // 新增（或替换了同槽位的上一世代条目）⇒ 有序索引失效。
            typed.order_dirty.set(true);
            let types = self.entity_components.entry(entity).or_default();
            let type_id = TypeId::of::<T>();
            if !types.contains(&type_id) {
                types.push(type_id);
            }
        }
        Ok(previous)
    }

    /// Reads a component attached to an entity.
    pub fn get<T: 'static>(&self, entity: EntityId) -> Result<Option<&T>, WorldError> {
        self.ensure_alive(entity)?;
        Ok(self
            .components
            .get(&TypeId::of::<T>())
            .and_then(|storage| storage.as_any().downcast_ref::<TypedStorage<T>>())
            .and_then(|storage| storage.dense.get(entity)))
    }

    /// Mutably reads a component attached to an entity.
    pub fn get_mut<T: 'static>(&mut self, entity: EntityId) -> Result<Option<&mut T>, WorldError> {
        self.ensure_alive(entity)?;
        Ok(self
            .components
            .get_mut(&TypeId::of::<T>())
            .and_then(|storage| storage.as_any_mut().downcast_mut::<TypedStorage<T>>())
            .and_then(|storage| storage.dense.get_mut(entity)))
    }

    /// Removes a component and returns its value, if it was present.
    pub fn remove<T: 'static>(&mut self, entity: EntityId) -> Result<Option<T>, WorldError> {
        self.ensure_alive(entity)?;
        let Some(storage) = self
            .components
            .get_mut(&TypeId::of::<T>())
            .and_then(|storage| storage.as_any_mut().downcast_mut::<TypedStorage<T>>())
        else {
            return Ok(None);
        };
        let removed = storage.dense.remove(entity);
        if removed.is_some() {
            storage.order_dirty.set(true);
            self.revision = self.revision.wrapping_add(1);
            if let Some(types) = self.entity_components.get_mut(&entity) {
                types.retain(|type_id| *type_id != TypeId::of::<T>());
            }
        }
        Ok(removed)
    }

    /// Returns whether an entity has a component of type `T`.
    pub fn has<T: 'static>(&self, entity: EntityId) -> Result<bool, WorldError> {
        self.ensure_alive(entity)?;
        Ok(self
            .components
            .get(&TypeId::of::<T>())
            .is_some_and(|storage| storage.contains_entity(entity)))
    }

    /// Returns all live entities that have component `T`.
    pub fn entities_with<T: 'static>(&self) -> Vec<EntityId> {
        self.components
            .get(&TypeId::of::<T>())
            .map_or_else(Vec::new, |storage| storage.entity_ids())
    }

    /// Returns all live entities that have both components `T` and `U`.
    pub fn entities_with2<T: 'static, U: 'static>(&self) -> Vec<EntityId> {
        let mut entities = self.entities_with::<T>();
        entities.retain(|entity| {
            self.components
                .get(&TypeId::of::<U>())
                .is_some_and(|storage| storage.contains_entity(*entity))
        });
        entities
    }

    /// Iterates all live entities with component `T` in sorted entity order.
    pub fn for_each<T: 'static>(&self, mut f: impl FnMut(EntityId, &T)) {
        let Some(storage) = self
            .components
            .get(&TypeId::of::<T>())
            .and_then(|storage| storage.as_any().downcast_ref::<TypedStorage<T>>())
        else {
            return;
        };
        storage.ensure_order();
        let order = storage.order.borrow();
        for &entity in order.iter() {
            if let Some(component) = storage.dense.get(entity) {
                f(entity, component);
            }
        }
    }

    /// Iterates all live entities with component `T` mutably in sorted entity order.
    pub fn for_each_mut<T: 'static>(&mut self, mut f: impl FnMut(EntityId, &mut T)) {
        let Some(storage) = self
            .components
            .get_mut(&TypeId::of::<T>())
            .and_then(|storage| storage.as_any_mut().downcast_mut::<TypedStorage<T>>())
        else {
            return;
        };
        storage.ensure_order_mut();
        // 取出有序索引再遍历：避免同时可变借用 `dense` 与不可变借用 `order`。
        // `mem::take` 是移动而不是克隆，遍历结束后放回，稳态帧因此零分配。
        let order = std::mem::take(&mut *storage.order.borrow_mut());
        for &entity in &order {
            if let Some(component) = storage.dense.get_mut(entity) {
                f(entity, component);
            }
        }
        *storage.order.borrow_mut() = order;
    }

    /// Iterates all live entities that have both components `T` and `U`.
    pub fn for_each2<T: 'static, U: 'static>(&self, mut f: impl FnMut(EntityId, &T, &U)) {
        let Some(left) = self
            .components
            .get(&TypeId::of::<T>())
            .and_then(|storage| storage.as_any().downcast_ref::<TypedStorage<T>>())
        else {
            return;
        };
        let Some(right) = self
            .components
            .get(&TypeId::of::<U>())
            .and_then(|storage| storage.as_any().downcast_ref::<TypedStorage<U>>())
        else {
            return;
        };
        if left.dense.len() <= right.dense.len() {
            left.ensure_order();
            let order = left.order.borrow();
            for &entity in order.iter() {
                if let Some(other) = right.dense.get(entity)
                    && let Some(component) = left.dense.get(entity)
                {
                    f(entity, component, other);
                }
            }
        } else {
            right.ensure_order();
            let order = right.order.borrow();
            for &entity in order.iter() {
                if let Some(component) = left.dense.get(entity)
                    && let Some(other) = right.dense.get(entity)
                {
                    f(entity, component, other);
                }
            }
        }
    }

    fn ensure_alive(&self, entity: EntityId) -> Result<(), WorldError> {
        self.is_alive(entity)
            .then_some(())
            .ok_or(WorldError::InvalidEntity(entity))
    }
}

#[cfg(test)]
mod tests {
    use super::{World, WorldError, order_rebuilds};
    use crate::entity::EntityId;

    #[derive(Debug, PartialEq)]
    struct Position {
        x: f32,
        y: f32,
    }

    #[derive(Debug, PartialEq)]
    struct Health(u32);

    #[test]
    fn creates_and_destroys_entities() {
        let mut world = World::new();
        let entity = world.spawn();
        assert!(world.is_alive(entity));
        assert_eq!(world.entity_count(), 1);
        assert_eq!(world.despawn(entity), Ok(()));
        assert!(!world.is_alive(entity));
        assert_eq!(world.entity_count(), 0);
        assert_eq!(
            world.despawn(entity),
            Err(WorldError::InvalidEntity(entity))
        );
    }

    #[test]
    fn inserts_reads_modifies_and_removes_components() {
        let mut world = World::new();
        let entity = world.spawn();
        assert_eq!(world.insert(entity, Position { x: 1.0, y: 2.0 }), Ok(None));
        assert_eq!(
            world.get(entity).unwrap(),
            Some(&Position { x: 1.0, y: 2.0 })
        );
        assert_eq!(
            world.insert(entity, Position { x: 3.0, y: 4.0 }),
            Ok(Some(Position { x: 1.0, y: 2.0 }))
        );
        world.get_mut::<Position>(entity).unwrap().unwrap().x = 9.0;
        assert_eq!(world.get::<Position>(entity).unwrap().unwrap().x, 9.0);
        assert!(world.has::<Position>(entity).unwrap());
        assert_eq!(
            world.remove::<Position>(entity),
            Ok(Some(Position { x: 9.0, y: 4.0 }))
        );
        assert!(!world.has::<Position>(entity).unwrap());
        assert_eq!(world.get::<Position>(entity).unwrap(), None);
    }

    #[test]
    fn despawn_removes_all_components() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 2.0 }).unwrap();
        world.insert(entity, Health(100)).unwrap();
        world.despawn(entity).unwrap();
        assert!(!world.entities_with::<Position>().contains(&entity));
        assert!(!world.entities_with::<Health>().contains(&entity));
        assert_eq!(
            world.get::<Position>(entity),
            Err(WorldError::InvalidEntity(entity))
        );
    }

    #[test]
    fn queries_component_combinations_in_sorted_entity_order() {
        let mut world = World::new();
        let first = world.spawn();
        let second = world.spawn();
        let third = world.spawn();
        world.insert(first, Position { x: 0.0, y: 0.0 }).unwrap();
        world.insert(second, Position { x: 1.0, y: 0.0 }).unwrap();
        world.insert(second, Health(10)).unwrap();
        world.insert(third, Health(20)).unwrap();
        assert_eq!(world.entities_with::<Position>(), vec![first, second]);
        assert_eq!(world.entities_with2::<Position, Health>(), vec![second]);
    }

    /// WP3：dense 存储的**有序 id 索引是惰性重建**的，因此"插入/删除打乱 dense 数组之后
    /// 遍历仍按 EntityId 升序"必须有测试钉住（这是替换前后唯一的行为契约风险点）。
    #[test]
    fn iteration_stays_sorted_after_interleaved_insert_and_remove() {
        let mut world = World::new();
        let ids = (0..8).map(|_| world.spawn()).collect::<Vec<_>>();
        for id in &ids {
            world.insert(*id, Health(id.index())).unwrap();
        }
        // 删中间几个，再补新实体（dense 会 `swap_remove`，顺序被打乱）。
        for id in [&ids[1], &ids[4], &ids[6]] {
            world.remove::<Health>(*id).unwrap();
        }
        let late = world.spawn();
        world.insert(late, Health(99)).unwrap();

        let mut seen: Vec<EntityId> = Vec::new();
        world.for_each::<Health>(|id, health| {
            seen.push(id);
            assert_eq!(health.0, if id == late { 99 } else { id.index() });
        });
        let mut expected = vec![ids[0], ids[2], ids[3], ids[5], ids[7], late];
        expected.sort_unstable();
        assert_eq!(seen, expected, "for_each 必须仍按 EntityId 升序");
        assert_eq!(world.entities_with::<Health>(), expected);

        // `for_each_mut` 走的是另一条借用路径，同样要有序。
        let mut mutated: Vec<EntityId> = Vec::new();
        world.for_each_mut::<Health>(|id, health| {
            mutated.push(id);
            health.0 += 1;
        });
        assert_eq!(mutated, expected, "for_each_mut 必须仍按 EntityId 升序");
        assert_eq!(world.get::<Health>(ids[0]).unwrap().unwrap().0, 1);

        // `for_each2` 在两侧规模不同的两个分支上都要有序。
        world.insert(ids[3], Position { x: 3.0, y: 0.0 }).unwrap();
        let mut pairs: Vec<EntityId> = Vec::new();
        world.for_each2::<Health, Position>(|id, _, _| pairs.push(id));
        assert_eq!(pairs, vec![ids[3]], "只有 ids[3] 同时有两个组件");

        // 重新插入已删除的组件后，索引同样要重建。
        world.insert(ids[1], Health(1)).unwrap();
        let mut restored: Vec<EntityId> = Vec::new();
        world.for_each::<Health>(|id, _| restored.push(id));
        let mut expected = expected.clone();
        expected.push(ids[1]);
        expected.sort_unstable();
        assert_eq!(restored, expected);
    }

    /// 契约回归：`for_each_mut` 曾**无条件**重排有序索引（`&self` 那一半有脏检查、`&mut` 这半
    /// 漏了），于是稳态帧每帧白做一次 O(n log n) 排序，正好违反 `TypedStorage` 注释里写下的
    /// 「稳态帧零分配」。两条断言方向相反、缺一不可：稳态**不许**重排（省掉的是真开销），
    /// 标脏后**必须**重排（否则读到过期顺序——不许用「干脆不排」换绿灯）。
    #[test]
    fn for_each_mut_rebuilds_order_only_when_dirty() {
        let mut world = World::new();
        for _ in 0..32 {
            let entity = world.spawn();
            world.insert(entity, Health(0)).unwrap();
        }

        let before = order_rebuilds();
        world.for_each_mut::<Health>(|_, _| {});
        assert_eq!(
            order_rebuilds() - before,
            1,
            "标脏后的首次 for_each_mut 必须重排（32 次插入只标脏，不重排就会读到过期顺序）"
        );

        let settled = order_rebuilds();
        for _ in 0..16 {
            world.for_each_mut::<Health>(|_, _| {});
        }
        assert_eq!(
            order_rebuilds(),
            settled,
            "稳态帧（无增删）一次都不许重排——契约见 TypedStorage 的 order 字段注释"
        );

        // 反向保护：插入必须重新标脏并被重建，否则新实体会漏出遍历。
        let late = world.spawn();
        world.insert(late, Health(7)).unwrap();
        let mut seen: Vec<EntityId> = Vec::new();
        world.for_each_mut::<Health>(|id, health| {
            seen.push(id);
            if id == late {
                assert_eq!(health.0, 7);
            }
        });
        assert_eq!(seen.len(), 33, "新插入的实体必须出现在遍历里");
        assert!(
            seen.windows(2).all(|pair| pair[0] < pair[1]),
            "仍须按 EntityId 升序"
        );
        assert_eq!(order_rebuilds() - settled, 1, "插入后必须恰好重排一次");

        // 删除同样要标脏。
        let after_insert = order_rebuilds();
        world.remove::<Health>(late).unwrap();
        let mut count = 0;
        world.for_each_mut::<Health>(|_, _| count += 1);
        assert_eq!(count, 32, "已删除的实体不许再出现在遍历里");
        assert_eq!(order_rebuilds() - after_insert, 1, "删除后必须恰好重排一次");
    }

    #[test]
    fn stale_handles_are_rejected_after_slot_reuse() {
        let mut world = World::new();
        let old_entity = world.spawn();
        world.insert(old_entity, Health(1)).unwrap();
        world.despawn(old_entity).unwrap();
        let new_entity = world.spawn();
        assert_ne!(old_entity, new_entity);
        assert_eq!(
            world.insert(old_entity, Health(2)),
            Err(WorldError::InvalidEntity(old_entity))
        );
        assert_eq!(world.get::<Health>(new_entity).unwrap(), None);
    }

    #[test]
    fn arbitrary_invalid_handles_are_rejected() {
        let world = World::new();
        let invalid = EntityId::from_raw(99, 1);
        assert_eq!(
            world.has::<Health>(invalid),
            Err(WorldError::InvalidEntity(invalid))
        );
    }
}
