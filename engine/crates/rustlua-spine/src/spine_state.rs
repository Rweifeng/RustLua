//! 跨帧的脚本可见状态：本帧刚触发、尚未被 `spine.events()` 取走的事件。
//! 与 `rustlua-animation::AnimationState` 同款「取走即清」语义。

use rustlua_ecs::entity::EntityId;
use rustlua_lua_api::Handle;

use crate::model::SpineEvent;

#[derive(Debug, Default)]
pub struct SpineState {
    events: Vec<(EntityId, SpineEvent)>,
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

    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.events.len()
    }

    /// 场景切换后旧实体 id 失效，事件必须随世界一起作废。
    pub fn clear_for_scene_transition(&mut self) {
        self.events.clear();
    }
}
