//! 相机选择：显式选中优先，否则排序序第一个**启用**的候选。
//!
//! 规则只有这一份（[`CameraSelection::resolve`] / [`CameraSelection::resolve_enabled`]），
//! 宿主只负责按自己的顺序产出候选（引擎里是 ECS 的排序实体序），因此结果确定。

/// 相机选择状态：显式选中的实体，没选就退回到"排序序第一个"。
///
/// 收敛前这段逻辑分别写在渲染器与宿主里（Lua 的 `camera.set_active` 与"首个组件"回退），
/// 多相机场景下"到底哪台生效"没有单一说法。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CameraSelection {
    pub active: Option<u64>,
}

impl CameraSelection {
    #[must_use]
    pub const fn new() -> Self {
        Self { active: None }
    }

    pub fn select(&mut self, entity: Option<u64>) {
        self.active = entity;
    }

    /// 这台实体是否是**显式选中**的那台。
    ///
    /// 宿主在"排序序遍历候选"时用它判断命中（遍历过程中"第一个"自然是回退项），
    /// 与 [`CameraSelection::resolve`] 是同一条规则的两种用法：都是"显式选中优先、
    /// 否则第一个"。
    #[must_use]
    pub const fn is_selected(self, entity: u64) -> bool {
        matches!(self.active, Some(active) if active == entity)
    }

    /// 从"排序序"的候选里挑出真正生效的那一个：优先显式选中，否则第一个。
    ///
    /// 候选顺序由调用方给出（引擎里是 ECS 的排序实体序），因此结果**确定**。
    #[must_use]
    pub fn resolve<T: Copy>(&self, candidates: impl IntoIterator<Item = (u64, T)>) -> Option<T> {
        self.resolve_enabled(candidates, |_| true)
    }

    /// [`CameraSelection::resolve`] 的**只挑启用项**版本（docs/54 §4.2 的 `enabled=false`）。
    ///
    /// `enabled` 为假的候选直接跳过：既不参与"第一个"回退，也不响应显式选中。
    /// 显式选中了一台被禁用的相机时返回下一个可用相机（或 `None`）——调用方据此记一条 warning：
    /// 静默用一台被关掉的相机（或静默什么都不画），会让"我明明关了它"变成一个查不出原因的现象。
    #[must_use]
    pub fn resolve_enabled<T: Copy>(
        &self,
        candidates: impl IntoIterator<Item = (u64, T)>,
        enabled: impl Fn(&T) -> bool,
    ) -> Option<T> {
        let mut fallback = None;
        for (entity, value) in candidates {
            if !enabled(&value) {
                continue;
            }
            if Some(entity) == self.active {
                return Some(value);
            }
            if fallback.is_none() {
                fallback = Some(value);
            }
        }
        fallback
    }
}
