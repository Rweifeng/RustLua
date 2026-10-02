//! docs/54 §4.4：运行时的**资源身份**（`AssetId`/`Handle`）。
//!
//! §4.4 要求"运行时统一使用 `AssetId`/`Handle`：SpriteRegion、Mesh、Texture、Material、
//! AnimationClip 和 Skeleton 共享资源身份系统"。此前运行时侧没有身份：每帧把用到的路径重新数一遍
//! 得到 `texture = 下标`（`RuntimeScene::texture_paths` 每帧清空重建），于是
//! **同一个下标在相邻两帧可以指两个完全不同的资源**，"这个 draw 引用的是哪个资源"跨帧不可比；
//! 资源被移除后也不可能发现"还有 draw 指着它"。
//!
//! 本模块给出两件事：
//!
//! - [`crate::asset_handle::AssetHandle`]：`(下标, 世代)` 的稳定身份。世代让**失效句柄**可判定——资源释放后
//!   `get(handle)` 返回 `None`，而不是拿到别人放在同一个下标上的资源（那正是最危险的一类错误：
//!   画面上出现一张"莫名其妙"的贴图）。
//! - [`crate::asset_handle::AssetStore`]：`key（路径）→ 句柄` 的注册表，`intern` 幂等，因此同一路径在进程内**永远**
//!   是同一个句柄。
//!
//! 与 `crate::resource_db`（编辑器/assetc 侧的**工程身份**：`AssetId` 是 16 字节内容指纹，随导入
//! 版本播种）刻意分开：那一层回答"这是哪个工程资源"，这一层回答"运行时这一帧引用的是哪一个已加载
//! 资源"。两者都不是对方的副本——运行时不查工程库，工程库也不认识运行时句柄。

use std::collections::HashMap;
use std::fmt;

use crate::resource_kind::ResourceKind;

/// 运行时的资源句柄：`(下标, 世代)`。
///
/// 同一个下标被复用（释放后重新 `intern`）时世代前进，因此**旧句柄不会指向新资源**。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AssetHandle {
    index: u32,
    generation: u32,
}

impl AssetHandle {
    #[must_use]
    pub const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

impl fmt::Display for AssetHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "asset#{}v{}", self.index, self.generation)
    }
}

/// 一条已注册的运行时资源：路径 + 粗粒度家族（与编辑器/assetc 的 `ResourceKind` 同一套说法）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetRecord {
    pub path: String,
    pub kind: ResourceKind,
}

struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

/// `key → 句柄` 的注册表。`intern` 幂等（同 key 恒同句柄），释放后旧句柄失效。
pub struct AssetStore<T> {
    slots: Vec<Slot<T>>,
    by_key: HashMap<String, AssetHandle>,
    live: usize,
}

impl<T> Default for AssetStore<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> AssetStore<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            by_key: HashMap::new(),
            live: 0,
        }
    }

    /// 注册一个 key：已有的 key 直接返回旧句柄（`created == false`），否则新建。
    ///
    /// 这是"跨帧稳定"的来源：同一路径在进程内只会有一个句柄。
    pub fn intern(&mut self, key: impl Into<String>, value: T) -> (AssetHandle, bool) {
        let key = key.into();
        if let Some(handle) = self.by_key.get(&key).copied() {
            return (handle, false);
        }
        let index = match self.slots.iter().position(|slot| slot.value.is_none()) {
            Some(index) => {
                // 复用下标：世代前进，因此指向旧句柄的引用不会看到新资源。
                let slot = &mut self.slots[index];
                slot.generation = slot.generation.wrapping_add(1);
                slot.value = Some(value);
                index
            }
            None => {
                self.slots.push(Slot {
                    generation: 1,
                    value: Some(value),
                });
                self.slots.len() - 1
            }
        };
        let handle = AssetHandle::new(index as u32, self.slots[index].generation);
        self.by_key.insert(key, handle);
        self.live += 1;
        (handle, true)
    }

    /// 取资源。**失效句柄返回 `None`**（下标被复用但世代不符）。
    #[must_use]
    pub fn get(&self, handle: AssetHandle) -> Option<&T> {
        let slot = self.slots.get(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.value.as_ref()
    }

    /// 该 key 当前的句柄（未注册则 `None`）。
    #[must_use]
    pub fn handle_of(&self, key: &str) -> Option<AssetHandle> {
        self.by_key.get(key).copied()
    }

    /// 释放资源：key 映射被移除、世代前进。返回是否真的释放了一个活着的资源。
    pub fn release(&mut self, handle: AssetHandle) -> bool {
        let Some(slot) = self.slots.get_mut(handle.index as usize) else {
            return false;
        };
        if slot.generation != handle.generation || slot.value.is_none() {
            return false;
        }
        slot.value = None;
        slot.generation = slot.generation.wrapping_add(1);
        self.by_key.retain(|_, mapped| *mapped != handle);
        self.live -= 1;
        true
    }

    /// 活着的资源数（不含已释放的槽）。
    #[must_use]
    pub const fn len(&self) -> usize {
        self.live
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// 所有活着的 `(句柄, 资源)`，按句柄排序（诊断/验收用；顺序稳定因此可直接断言）。
    #[must_use]
    pub fn entries(&self) -> Vec<(AssetHandle, &T)> {
        let mut entries: Vec<(AssetHandle, &T)> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.value
                    .as_ref()
                    .map(|value| (AssetHandle::new(index as u32, slot.generation), value))
            })
            .collect();
        entries.sort_by_key(|(handle, _)| *handle);
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(path: &str) -> AssetRecord {
        AssetRecord {
            path: path.to_owned(),
            kind: ResourceKind::from_path(path),
        }
    }

    /// 同一路径在进程内恒同句柄、不同路径不同句柄——"跨帧可比"就建立在这条上。
    #[test]
    fn intern_is_idempotent_and_stable_across_calls() {
        let mut store: AssetStore<AssetRecord> = AssetStore::new();
        let (first, created) = store.intern("a.png", record("a.png"));
        assert!(created);
        let (again, created) = store.intern("a.png", record("a.png"));
        assert!(!created, "重复 intern 不该新建");
        assert_eq!(first, again);
        let (other, _) = store.intern("b.png", record("b.png"));
        assert_ne!(first, other);
        assert_eq!(store.len(), 2);
        assert_eq!(store.handle_of("a.png"), Some(first));
        assert_eq!(
            store.get(first).map(|record| record.path.as_str()),
            Some("a.png")
        );
        assert_eq!(
            store.get(first).map(|record| record.kind),
            Some(ResourceKind::Texture)
        );
    }

    /// 释放后**旧句柄必须失效**，即使下标被复用：否则画面上会出现"莫名其妙"的贴图。
    #[test]
    fn released_handles_go_stale_and_never_alias_the_reused_slot() {
        let mut store: AssetStore<AssetRecord> = AssetStore::new();
        let (first, _) = store.intern("a.png", record("a.png"));
        assert!(store.release(first));
        assert!(store.get(first).is_none(), "释放后旧句柄必须取不到");
        assert!(store.is_empty());
        assert_eq!(store.handle_of("a.png"), None);

        let (reused, created) = store.intern("b.png", record("b.png"));
        assert!(created);
        assert_eq!(
            reused.index(),
            first.index(),
            "这里刻意复用下标，好让世代检查真的被走到"
        );
        assert_ne!(reused.generation(), first.generation());
        assert!(
            store.get(first).is_none(),
            "复用了下标也不能让旧句柄看到新资源"
        );
        assert_eq!(
            store.get(reused).map(|record| record.path.as_str()),
            Some("b.png")
        );
    }

    /// 重复释放、释放不存在的句柄都必须是空操作（不是 panic，也不是把别的资源放掉）。
    #[test]
    fn release_is_idempotent_and_ignores_stale_handles() {
        let mut store: AssetStore<AssetRecord> = AssetStore::new();
        let (handle, _) = store.intern("a.png", record("a.png"));
        assert!(store.release(handle));
        assert!(!store.release(handle), "重复释放应为 no-op");
        assert!(
            !store.release(AssetHandle::new(99, 1)),
            "越界句柄应为 no-op"
        );
        let (fresh, _) = store.intern("c.png", record("c.png"));
        assert!(!store.release(handle), "旧世代句柄不该释放新资源");
        assert!(store.get(fresh).is_some());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn entries_are_sorted_by_handle() {
        let mut store: AssetStore<AssetRecord> = AssetStore::new();
        let (a, _) = store.intern("a.png", record("a.png"));
        let (b, _) = store.intern("b.png", record("b.png"));
        let entries = store.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, a);
        assert_eq!(entries[1].0, b);
    }
}
