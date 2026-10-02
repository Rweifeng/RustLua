//! WP3 原型：dense 值数组 + sparse 槽位索引的组件存储。
//!
//! 现状（`core/world.rs`）是 `HashMap<EntityId, T>` 加一份排序的 `Vec<EntityId>`：查询要
//! 哈希两次再加 downcast，插入/删除是 O(n) memmove，遍历按哈希桶跳。本模块提供替代实现，
//! 用 A/B 基准证明收益后再决定是否替换（docs/40 §3 WP3）。
//!
//! **顺序语义与现状不同**：dense 遍历按插入/交换顺序，**不是** EntityId 有序。需要确定性
//! 的边界（快照、序列化、规则回归）必须显式调 [`crate::dense_storage::DenseStorage::sorted`]——现有
//! `World::for_each` 的"有序"契约**不能**直接照搬到热遍历上（docs/40 §3 WP3）。

use crate::entity::EntityId;

/// sparse 里的空槽哨兵。用 `u32` 哨兵而不是 `Option<u32>`：后者因对齐占 8 字节，
/// 10 万实体时白吃约 800 KB。
const EMPTY: u32 = u32::MAX;

/// 以 dense 值数组为主体的组件存储。
///
/// `values` 与 `entities` 平行；`sparse` 把 `EntityId::index()` 映射到 dense 下标。
/// 删除用 `swap_remove`，因此增删查都是 O(1)，遍历是连续内存扫描。
#[derive(Debug)]
pub struct DenseStorage<T> {
    values: Vec<T>,
    entities: Vec<EntityId>,
    sparse: Vec<u32>,
}

impl<T> Default for DenseStorage<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> DenseStorage<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            values: Vec::new(),
            entities: Vec::new(),
            sparse: Vec::new(),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    #[must_use]
    pub fn contains(&self, entity: EntityId) -> bool {
        self.dense_index(entity).is_some()
    }

    #[must_use]
    pub fn get(&self, entity: EntityId) -> Option<&T> {
        self.values.get(self.dense_index(entity)? as usize)
    }

    pub fn get_mut(&mut self, entity: EntityId) -> Option<&mut T> {
        let dense = self.dense_index(entity)? as usize;
        self.values.get_mut(dense)
    }

    /// 插入或替换。返回被替换的旧值。
    ///
    /// 槽位被**上一世代**占用时（调用方本应先校验存活）会先清掉旧条目：同槽位留两条
    /// 记录会让 sparse 只能指向其中一条，另一条变成查不到的幽灵。
    pub fn insert(&mut self, entity: EntityId, value: T) -> Option<T> {
        let slot = entity.index() as usize;
        if self.sparse.len() <= slot {
            self.sparse.resize(slot + 1, EMPTY);
        }
        let occupied = self.sparse[slot];
        if occupied != EMPTY {
            let existing = self.entities[occupied as usize];
            if existing == entity {
                return Some(std::mem::replace(
                    &mut self.values[occupied as usize],
                    value,
                ));
            }
            self.remove(existing);
        }
        self.sparse[slot] = self.values.len() as u32;
        self.entities.push(entity);
        self.values.push(value);
        None
    }

    /// 删除并返回组件。O(1)：`swap_remove` 之后必须把被搬动实体的 sparse 指回新位置，
    /// 漏掉这一步就会留下"查得到但值已换"的错位条目。
    pub fn remove(&mut self, entity: EntityId) -> Option<T> {
        let dense = self.dense_index(entity)? as usize;
        let removed = self.values.swap_remove(dense);
        self.entities.swap_remove(dense);
        self.sparse[entity.index() as usize] = EMPTY;
        // `swap_remove` 返回的是**被删元素**，不是被搬过来的那个；搬过来的是现在位于
        // `dense` 的元素。它的 sparse 必须指回新位置，否则会指向已失效的旧下标。
        if dense < self.entities.len() {
            let moved = self.entities[dense];
            self.sparse[moved.index() as usize] = dense as u32;
        }
        Some(removed)
    }

    /// dense 顺序遍历（插入/交换顺序，**非** EntityId 有序）。零分配。
    pub fn iter(&self) -> impl Iterator<Item = (EntityId, &T)> + '_ {
        self.entities.iter().copied().zip(self.values.iter())
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (EntityId, &mut T)> + '_ {
        self.entities.iter().copied().zip(self.values.iter_mut())
    }

    /// 连续的值切片：批量数值运算（物理回写、动画采样、剔除）直接吃这一份。
    #[must_use]
    pub fn values(&self) -> &[T] {
        &self.values
    }

    #[must_use]
    pub fn entities(&self) -> &[EntityId] {
        &self.entities
    }

    /// 按 EntityId 有序的一览：仅供快照/序列化等确定性边界使用。
    #[must_use]
    pub fn sorted(&self) -> Vec<(EntityId, &T)> {
        let mut entries = self.iter().collect::<Vec<_>>();
        entries.sort_unstable_by_key(|(entity, _)| *entity);
        entries
    }

    fn dense_index(&self, entity: EntityId) -> Option<u32> {
        let dense = *self.sparse.get(entity.index() as usize)?;
        if dense == EMPTY {
            return None;
        }
        // 槽位复用后旧句柄必须失效：dense 存的是完整句柄（含 generation），比对一次即可。
        (self.entities[dense as usize] == entity).then_some(dense)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(index: u32) -> EntityId {
        EntityId::from_raw(index, EntityId::INITIAL_GENERATION)
    }

    #[test]
    fn inserts_reads_replaces_and_removes() {
        let mut storage = DenseStorage::new();
        assert!(storage.is_empty());
        assert_eq!(storage.insert(entity(0), 10), None);
        assert_eq!(storage.insert(entity(1), 20), None);
        assert_eq!(storage.len(), 2);
        assert_eq!(storage.get(entity(0)), Some(&10));
        assert_eq!(storage.get(entity(1)), Some(&20));
        assert!(storage.contains(entity(0)));

        assert_eq!(storage.insert(entity(0), 11), Some(10));
        assert_eq!(storage.get(entity(0)), Some(&11));
        assert_eq!(storage.len(), 2);

        assert_eq!(storage.remove(entity(0)), Some(11));
        assert_eq!(storage.remove(entity(0)), None);
        assert_eq!(storage.len(), 1);
        assert!(!storage.contains(entity(0)));
    }

    /// `swap_remove` 后必须修正被搬动实体的 sparse 指向——这是 dense/sparse 最经典的
    /// 错位 bug，专门锁一条。
    #[test]
    fn swap_remove_keeps_other_entities_reachable() {
        let mut storage = DenseStorage::new();
        for index in 0..4 {
            storage.insert(entity(index), index as i32);
        }
        // 删中间一个：最后一个会被搬到它的位置。
        assert_eq!(storage.remove(entity(1)), Some(1));
        assert_eq!(storage.len(), 3);
        for index in [0_u32, 2, 3] {
            assert_eq!(
                storage.get(entity(index)),
                Some(&(index as i32)),
                "实体 {index} 在 swap_remove 后错位"
            );
        }
        assert_eq!(storage.get(entity(1)), None);

        // 删最后一个：不应触发搬动，也不该污染别的槽位。
        assert_eq!(storage.remove(entity(3)), Some(3));
        assert_eq!(storage.len(), 2);
        assert_eq!(storage.get(entity(0)), Some(&0));
        assert_eq!(storage.get(entity(2)), Some(&2));
    }

    #[test]
    fn stale_generation_handles_are_rejected() {
        let mut storage = DenseStorage::new();
        storage.insert(EntityId::from_raw(0, 1), 1);
        assert_eq!(storage.get(EntityId::from_raw(0, 2)), None);
        assert_eq!(storage.remove(EntityId::from_raw(0, 2)), None);
        assert_eq!(storage.len(), 1);
    }

    #[test]
    fn reusing_a_slot_drops_the_previous_generation_entry() {
        let mut storage = DenseStorage::new();
        storage.insert(EntityId::from_raw(0, 1), 1);
        storage.insert(EntityId::from_raw(0, 2), 2);
        assert_eq!(storage.len(), 1, "同槽位不得留下两条记录");
        assert_eq!(storage.get(EntityId::from_raw(0, 1)), None);
        assert_eq!(storage.get(EntityId::from_raw(0, 2)), Some(&2));
    }

    #[test]
    fn iteration_covers_exactly_the_live_set() {
        let mut storage = DenseStorage::new();
        for index in 0..64 {
            storage.insert(entity(index), index as i32);
        }
        // 反复删/插，制造 dense 数组乱序。
        for index in (0..64).step_by(3) {
            storage.remove(entity(index));
        }
        for index in 100..110 {
            storage.insert(entity(index), index as i32);
        }
        let live = (0..64)
            .filter(|index| index % 3 != 0)
            .chain(100..110)
            .collect::<Vec<_>>();
        assert_eq!(storage.len(), live.len());
        for (seen, (id, _)) in storage.iter().enumerate() {
            let _ = seen;
            assert!(storage.contains(id));
        }
        assert_eq!(storage.iter().count(), live.len());
        assert_eq!(storage.iter_mut().count(), live.len());
        for index in &live {
            assert_eq!(storage.get(entity(*index)), Some(&(*index as i32)));
        }
        for index in (0..64).step_by(3) {
            assert_eq!(storage.get(entity(index)), None);
        }
        assert_eq!(storage.values().len(), live.len());
        assert_eq!(storage.entities().len(), live.len());
    }

    #[test]
    fn sorted_is_ascending_by_entity_id() {
        let mut storage = DenseStorage::new();
        for index in [7_u32, 2, 9, 4] {
            storage.insert(entity(index), index as i32);
        }
        let ids = storage
            .sorted()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        assert_eq!(ids, [entity(2), entity(4), entity(7), entity(9)]);
    }
}
