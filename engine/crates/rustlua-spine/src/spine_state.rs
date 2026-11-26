//! 跨帧的脚本可见状态：本帧刚触发、尚未被 `spine.events()` 取走的事件。
//! 与 `rustlua-animation::AnimationState` 同款「取走即清」语义。

use std::path::PathBuf;

use rustlua_ecs::entity::EntityId;
use rustlua_lua_api::Handle;

use crate::model::SpineEvent;

#[derive(Debug, Default)]
pub struct SpineState {
    events: Vec<(EntityId, SpineEvent)>,
    /// 工程根，供 `spine.snapshot` 解析图集页的工程内路径（M85 G03）。
    ///
    /// 组合根在装配期写入；`rustlua-spine` 自己不知道工程根在哪，而快照要把
    /// 图集 PNG 读进内存做软栅格化，必须能定位到它。
    project_root: Option<PathBuf>,
}

impl SpineState {
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "spine")
    }

    pub fn push_events(&mut self, events: Vec<(EntityId, SpineEvent)>) {
        self.events.extend(events);
    }

    pub fn take_events(&mut self) -> Vec<(EntityId, SpineEvent)> {
        std::mem::take(&mut self.events)
    }

    /// 由组合根在装配期写入工程根（`spine.snapshot` 读图集时用）。
    pub fn set_project_root(&mut self, root: PathBuf) {
        self.project_root = Some(root);
    }

    #[must_use]
    pub fn project_root(&self) -> Option<&std::path::Path> {
        self.project_root.as_deref()
    }

    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.events.len()
    }

    /// 场景切换后旧实体 id 失效，事件必须随世界一起作废。
    pub fn clear_for_scene_transition(&mut self) {
        self.events.clear();
    }
}
