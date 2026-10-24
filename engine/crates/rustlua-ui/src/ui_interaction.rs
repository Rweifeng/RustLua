//! M76-02: runtime UI interaction closure.
//!
//! Pure, host-agnostic tree used by all three hosts: hit testing in paint order,
//! capture/bubble event routing, hover/pressed/focus/disabled states, pointer
//! capture, scroll offsets with rubber-band rebound, and stable [`UiKey`]s.
//!
//! The tree is rebuilt from layout each frame by the host; this module owns the
//! cross-frame interaction state (hover, press, focus, scroll) so PC/Web/WeChat
//! share one dispatch order instead of three ad-hoc implementations.

use crate::ui_events::UiEventType;
use crate::ui_states::{ButtonStateSet, ButtonVisualState};
use std::collections::BTreeMap;

/// Stable identity for a Spec node key. Runtime tooling and goldens key off this
/// value, so it must not depend on tree position or frame number.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct UiKey(u64);

impl UiKey {
    /// FNV-1a over the node key. Deterministic across hosts and process runs.
    #[must_use]
    pub fn from_node_key(key: &str) -> Self {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in key.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        Self(hash)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Screen-space rectangle (y-down, pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl UiRect {
    /// 空矩形：与任何矩形求交都为空，可用于「被裁剪掉」的占位。
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    #[must_use]
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            x,
            y,
            w: w.max(0.0),
            h: h.max(0.0),
        }
    }

    #[must_use]
    pub fn contains(self, point: [f32; 2]) -> bool {
        point[0] >= self.x
            && point[0] < self.x + self.w
            && point[1] >= self.y
            && point[1] < self.y + self.h
    }

    #[must_use]
    pub fn intersect(self, other: Self) -> Option<Self> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        (x1 > x0 && y1 > y0).then(|| Self::new(x0, y0, x1 - x0, y1 - y0))
    }

    #[must_use]
    pub fn translated(self, delta: [f32; 2]) -> Self {
        Self::new(self.x + delta[0], self.y + delta[1], self.w, self.h)
    }
}

/// Scroll range for a `ScrollView` node. `viewport` is the node's own rect size
/// on the scrolling axis; callers may override it via [`Self::with_viewport`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiScroll {
    pub horizontal: bool,
    pub content: f32,
    pub offset: f32,
    viewport: Option<f32>,
}

impl UiScroll {
    #[must_use]
    pub fn vertical(content: f32) -> Self {
        Self {
            horizontal: false,
            content: content.max(0.0),
            offset: 0.0,
            viewport: None,
        }
    }

    #[must_use]
    pub fn horizontal(content: f32) -> Self {
        Self {
            horizontal: true,
            content: content.max(0.0),
            offset: 0.0,
            viewport: None,
        }
    }

    #[must_use]
    pub fn with_viewport(mut self, viewport: f32) -> Self {
        self.viewport = Some(viewport.max(0.0));
        self
    }

    #[must_use]
    pub fn max_offset(&self, node: UiRect) -> f32 {
        let viewport = self
            .viewport
            .unwrap_or(if self.horizontal { node.w } else { node.h });
        (self.content - viewport).max(0.0)
    }
}

/// One node in the interaction tree. `clips_content` is set for `ScrollView`
/// (and modal masks); children are hit-tested and drawn only inside it.
#[derive(Clone, Debug)]
pub struct UiInteractionNode {
    pub key: String,
    pub rect: UiRect,
    pub enabled: bool,
    pub focusable: bool,
    /// Overlay/decorative nodes opt out of pointer hit testing.
    pub pointer_events: bool,
    pub states: Option<ButtonStateSet>,
    pub clips_content: bool,
    pub scroll: Option<UiScroll>,
}

impl UiInteractionNode {
    #[must_use]
    pub fn new(key: impl Into<String>, rect: UiRect) -> Self {
        Self {
            key: key.into(),
            rect,
            enabled: true,
            focusable: false,
            pointer_events: true,
            states: None,
            clips_content: false,
            scroll: None,
        }
    }

    #[must_use]
    pub fn focusable(mut self, focusable: bool) -> Self {
        self.focusable = focusable;
        self
    }

    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.enabled = !disabled;
        self
    }

    #[must_use]
    pub fn pointer_events(mut self, pointer_events: bool) -> Self {
        self.pointer_events = pointer_events;
        self
    }

    #[must_use]
    pub fn states(mut self, states: ButtonStateSet) -> Self {
        self.states = Some(states);
        self
    }

    #[must_use]
    pub fn clips(mut self, clips: bool) -> Self {
        self.clips_content = clips;
        self
    }

    #[must_use]
    pub fn scroll(mut self, scroll: UiScroll) -> Self {
        self.scroll = Some(scroll);
        self.clips_content = true;
        self
    }
}

/// Event routing phase. Capture walks root→parent, `Target` is the hit node,
/// bubble walks parent→root (the target is not repeated).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiPhase {
    Capture,
    Target,
    Bubble,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiDispatch {
    pub phase: UiPhase,
    pub key: String,
    pub event: UiEventType,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiNavKey {
    Tab,
    ShiftTab,
    Enter,
    Escape,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiOutcome {
    Dispatch(UiDispatch),
    StateChanged {
        key: String,
        state: ButtonVisualState,
    },
    FocusChanged {
        from: Option<String>,
        to: Option<String>,
    },
    Scrolled {
        key: String,
        offset: f32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum UiInteractionError {
    #[error("interaction parent {0} does not exist")]
    UnknownParent(usize),
    #[error("duplicate interaction node key: {0}")]
    DuplicateKey(String),
    #[error("UiKey collision between {existing:?} and {incoming:?}")]
    KeyCollision { existing: String, incoming: String },
}

/// Duplicate/collision detection for `UiKey`. Missing nodes must surface as a
/// diagnostic upstream, never as a silent no-op.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiKeyIndex {
    by_id: BTreeMap<UiKey, String>,
}

impl UiKeyIndex {
    pub fn insert(&mut self, node_key: &str) -> Result<UiKey, UiInteractionError> {
        let id = UiKey::from_node_key(node_key);
        if let Some(existing) = self.by_id.get(&id) {
            return if existing == node_key {
                Err(UiInteractionError::DuplicateKey(node_key.to_owned()))
            } else {
                Err(UiInteractionError::KeyCollision {
                    existing: existing.clone(),
                    incoming: node_key.to_owned(),
                })
            };
        }
        self.by_id.insert(id, node_key.to_owned());
        Ok(id)
    }

    #[must_use]
    pub fn id_for(&self, node_key: &str) -> Option<UiKey> {
        self.by_id
            .iter()
            .find_map(|(id, key)| (key == node_key).then_some(*id))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[derive(Clone, Debug)]
struct NodeState {
    node: UiInteractionNode,
    parent: Option<usize>,
    children: Vec<usize>,
    clip: Option<UiRect>,
    visual: ButtonVisualState,
}

#[derive(Clone, Copy, Debug)]
struct ScrollDrag {
    scroll: usize,
    start: [f32; 2],
    start_offset: f32,
    dragging: bool,
}

/// Cross-frame interaction state for one UI screen.
#[derive(Clone, Debug)]
pub struct UiInteraction {
    nodes: Vec<NodeState>,
    roots: Vec<usize>,
    by_key: BTreeMap<String, usize>,
    hovered: Option<usize>,
    pressed: Option<usize>,
    focused: Option<usize>,
    drag: Option<ScrollDrag>,
    drag_threshold: f32,
}

impl Default for UiInteraction {
    fn default() -> Self {
        Self::new()
    }
}

impl UiInteraction {
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            roots: Vec::new(),
            by_key: BTreeMap::new(),
            hovered: None,
            pressed: None,
            focused: None,
            drag: None,
            drag_threshold: 8.0,
        }
    }

    pub fn set_drag_threshold(&mut self, pixels: f32) {
        self.drag_threshold = pixels.max(0.0);
    }

    /// Adds a node under `parent` (`None` = screen root). Paint order is push
    /// order: later siblings are on top.
    pub fn push_node(
        &mut self,
        parent: Option<usize>,
        node: UiInteractionNode,
    ) -> Result<usize, UiInteractionError> {
        if let Some(parent) = parent
            && parent >= self.nodes.len()
        {
            return Err(UiInteractionError::UnknownParent(parent));
        }
        if self.by_key.contains_key(&node.key) {
            return Err(UiInteractionError::DuplicateKey(node.key));
        }
        let clip = match parent {
            Some(parent) => {
                let parent_clip = self.nodes[parent].clip;
                let parent_rect = self.nodes[parent].node.rect;
                if self.nodes[parent].node.clips_content {
                    match parent_clip {
                        Some(clip) => clip.intersect(parent_rect),
                        None => Some(parent_rect),
                    }
                } else {
                    parent_clip
                }
            }
            None => None,
        };
        let index = self.nodes.len();
        self.by_key.insert(node.key.clone(), index);
        self.nodes.push(NodeState {
            node,
            parent,
            children: Vec::new(),
            clip,
            visual: ButtonVisualState::Normal,
        });
        match parent {
            Some(parent) => self.nodes[parent].children.push(index),
            None => self.roots.push(index),
        }
        Ok(index)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    #[must_use]
    pub fn node(&self, index: usize) -> Option<&UiInteractionNode> {
        self.nodes.get(index).map(|state| &state.node)
    }

    #[must_use]
    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.by_key.get(key).copied()
    }

    #[must_use]
    pub fn visual_state(&self, index: usize) -> Option<ButtonVisualState> {
        self.nodes.get(index).map(|state| state.visual)
    }

    #[must_use]
    pub fn focused_key(&self) -> Option<&str> {
        self.focused
            .and_then(|index| self.nodes.get(index))
            .map(|state| state.node.key.as_str())
    }

    #[must_use]
    pub fn hovered_key(&self) -> Option<&str> {
        self.hovered
            .and_then(|index| self.nodes.get(index))
            .map(|state| state.node.key.as_str())
    }

    #[must_use]
    pub fn scroll_offset(&self, key: &str) -> Option<f32> {
        let index = *self.by_key.get(key)?;
        self.nodes[index]
            .node
            .scroll
            .as_ref()
            .map(|scroll| scroll.offset)
    }

    /// Sum of ancestor scroll offsets; scrolling moves content, not the viewport.
    fn scroll_shift(&self, index: usize) -> [f32; 2] {
        let mut shift = [0.0; 2];
        let mut parent = self.nodes[index].parent;
        while let Some(current) = parent {
            if let Some(scroll) = &self.nodes[current].node.scroll {
                if scroll.horizontal {
                    shift[0] -= scroll.offset;
                } else {
                    shift[1] -= scroll.offset;
                }
            }
            parent = self.nodes[current].parent;
        }
        shift
    }

    #[must_use]
    pub fn effective_rect(&self, index: usize) -> Option<UiRect> {
        self.nodes
            .get(index)
            .map(|state| state.node.rect.translated(self.scroll_shift(index)))
    }

    fn effective_enabled(&self, index: usize) -> bool {
        let mut current = Some(index);
        while let Some(index) = current {
            let state = &self.nodes[index];
            if !state.node.enabled {
                return false;
            }
            current = state.parent;
        }
        true
    }

    /// Deepest node at `point`, honouring paint order, clips, pointer opt-outs
    /// and disabled subtrees.
    #[must_use]
    pub fn hit_test(&self, point: [f32; 2]) -> Option<usize> {
        for index in (0..self.nodes.len()).rev() {
            let state = &self.nodes[index];
            if !state.node.pointer_events || !self.effective_enabled(index) {
                continue;
            }
            if state.clip.is_some_and(|clip| !clip.contains(point)) {
                continue;
            }
            if self
                .effective_rect(index)
                .is_some_and(|rect| rect.contains(point))
            {
                return Some(index);
            }
        }
        None
    }

    fn path_to(&self, index: usize) -> Vec<usize> {
        let mut path = Vec::new();
        let mut current = Some(index);
        while let Some(index) = current {
            path.push(index);
            current = self.nodes[index].parent;
        }
        path.reverse();
        path
    }

    fn dispatch(&self, index: usize, event: UiEventType, out: &mut Vec<UiOutcome>) {
        let path = self.path_to(index);
        for &ancestor in &path[..path.len().saturating_sub(1)] {
            out.push(UiOutcome::Dispatch(UiDispatch {
                phase: UiPhase::Capture,
                key: self.nodes[ancestor].node.key.clone(),
                event,
            }));
        }
        out.push(UiOutcome::Dispatch(UiDispatch {
            phase: UiPhase::Target,
            key: self.nodes[index].node.key.clone(),
            event,
        }));
        for &ancestor in path[..path.len().saturating_sub(1)].iter().rev() {
            out.push(UiOutcome::Dispatch(UiDispatch {
                phase: UiPhase::Bubble,
                key: self.nodes[ancestor].node.key.clone(),
                event,
            }));
        }
    }

    fn desired_state(&self, index: usize) -> ButtonVisualState {
        if !self.effective_enabled(index) {
            return ButtonVisualState::Disabled;
        }
        if self.pressed == Some(index) {
            return ButtonVisualState::Pressed;
        }
        if self.hovered == Some(index) {
            return ButtonVisualState::Hover;
        }
        if self.focused == Some(index) {
            return ButtonVisualState::Focused;
        }
        ButtonVisualState::Normal
    }

    fn refresh_states(&mut self, out: &mut Vec<UiOutcome>) {
        for index in 0..self.nodes.len() {
            if self.nodes[index].node.states.is_none() {
                continue;
            }
            let desired = self.desired_state(index);
            if self.nodes[index].visual != desired {
                self.nodes[index].visual = desired;
                out.push(UiOutcome::StateChanged {
                    key: self.nodes[index].node.key.clone(),
                    state: desired,
                });
            }
        }
    }

    fn set_hovered(&mut self, next: Option<usize>, out: &mut Vec<UiOutcome>) {
        if self.hovered == next {
            return;
        }
        if let Some(previous) = self.hovered {
            self.dispatch(previous, UiEventType::PointerExit, out);
        }
        self.hovered = next;
        if let Some(next) = next {
            self.dispatch(next, UiEventType::PointerEnter, out);
        }
    }

    fn set_focus(&mut self, next: Option<usize>, out: &mut Vec<UiOutcome>) {
        if self.focused == next {
            return;
        }
        let from = self
            .focused
            .and_then(|index| self.nodes.get(index))
            .map(|state| state.node.key.clone());
        let to = next.map(|index| self.nodes[index].node.key.clone());
        self.focused = next;
        out.push(UiOutcome::FocusChanged { from, to });
        if let Some(next) = next {
            self.dispatch(next, UiEventType::Focus, out);
        }
    }

    /// ScrollView ancestor (or self) of `index`.
    fn scroll_ancestor(&self, index: usize) -> Option<usize> {
        let mut current = Some(index);
        while let Some(index) = current {
            if self.nodes[index].node.scroll.is_some() {
                return Some(index);
            }
            current = self.nodes[index].parent;
        }
        None
    }

    fn scroll_to(&mut self, index: usize, offset: f32, out: &mut Vec<UiOutcome>) -> f32 {
        let Some(scroll) = &mut self.nodes[index].node.scroll else {
            return 0.0;
        };
        scroll.offset = offset;
        out.push(UiOutcome::Scrolled {
            key: self.nodes[index].node.key.clone(),
            offset,
        });
        offset
    }

    pub fn pointer_move(&mut self, point: [f32; 2], out: &mut Vec<UiOutcome>) {
        let hit = self.hit_test(point);
        self.set_hovered(hit, out);
        if let Some(drag) = self.drag
            && self.pressed.is_some()
        {
            let axis = if self
                .nodes
                .get(drag.scroll)
                .and_then(|state| state.node.scroll.as_ref())
                .is_some_and(|scroll| scroll.horizontal)
            {
                0
            } else {
                1
            };
            let delta = point[axis] - drag.start[axis];
            if !drag.dragging
                && delta.abs() >= self.drag_threshold
                && let Some(drag) = &mut self.drag
            {
                drag.dragging = true;
            }
            if self.drag.is_some_and(|drag| drag.dragging) {
                let max = self
                    .nodes
                    .get(drag.scroll)
                    .and_then(|state| {
                        state
                            .node
                            .scroll
                            .as_ref()
                            .map(|scroll| scroll.max_offset(state.node.rect))
                    })
                    .unwrap_or(0.0);
                let raw = drag.start_offset - delta;
                // Rubber-band outside the valid range; `rebound` settles it back.
                let offset = if raw < 0.0 {
                    raw * 0.35
                } else if raw > max {
                    max + (raw - max) * 0.35
                } else {
                    raw
                };
                self.scroll_to(drag.scroll, offset, out);
            }
        }
        self.refresh_states(out);
    }

    pub fn pointer_down(&mut self, point: [f32; 2], out: &mut Vec<UiOutcome>) {
        let hit = self.hit_test(point);
        self.set_hovered(hit, out);
        let Some(index) = hit else {
            self.pressed = None;
            self.drag = None;
            self.refresh_states(out);
            return;
        };
        self.pressed = Some(index);
        self.drag = self.scroll_ancestor(index).and_then(|scroll| {
            self.nodes[scroll]
                .node
                .scroll
                .as_ref()
                .map(|state| ScrollDrag {
                    scroll,
                    start: point,
                    start_offset: state.offset,
                    dragging: false,
                })
        });
        if self.nodes[index].node.focusable {
            self.set_focus(Some(index), out);
        }
        self.dispatch(index, UiEventType::PointerDown, out);
        self.refresh_states(out);
    }

    pub fn pointer_up(&mut self, point: [f32; 2], out: &mut Vec<UiOutcome>) {
        let Some(pressed) = self.pressed.take() else {
            self.drag = None;
            return;
        };
        let dragging = self.drag.is_some_and(|drag| drag.dragging);
        self.drag = None;
        let hit = self.hit_test(point);
        self.set_hovered(hit, out);
        self.dispatch(pressed, UiEventType::PointerUp, out);
        if !dragging && hit == Some(pressed) {
            self.dispatch(pressed, UiEventType::Click, out);
        }
        self.refresh_states(out);
    }

    /// Wheel over the deepest scrollable node under the pointer.
    pub fn wheel(&mut self, point: [f32; 2], delta: f32, out: &mut Vec<UiOutcome>) {
        self.set_hovered(self.hit_test(point), out);
        let Some(hit) = self.hovered else {
            return;
        };
        let Some(scroll_index) = self.scroll_ancestor(hit) else {
            return;
        };
        let Some((current, max)) = self.nodes[scroll_index].node.scroll.as_ref().map(|scroll| {
            (
                scroll.offset,
                scroll.max_offset(self.nodes[scroll_index].node.rect),
            )
        }) else {
            return;
        };
        let offset = (current + delta).clamp(0.0, max);
        self.scroll_to(scroll_index, offset, out);
    }

    /// Dispatches `Click` at `index` without a pointer gesture. The host uses this
    /// to preserve the engine's legacy "click on press" contract while still
    /// routing hover/press state through this tree; Enter uses the same path.
    pub fn activate(&mut self, index: usize, out: &mut Vec<UiOutcome>) {
        if index < self.nodes.len() {
            self.dispatch(index, UiEventType::Click, out);
        }
    }

    /// Keyboard navigation minimal set: Tab/Shift+Tab move focus, Enter activates
    /// the focused node, Escape clears focus. Disabled nodes are skipped.
    pub fn key(&mut self, key: UiNavKey, out: &mut Vec<UiOutcome>) {
        match key {
            UiNavKey::Tab | UiNavKey::ShiftTab => {
                let order: Vec<usize> = (0..self.nodes.len())
                    .filter(|&index| {
                        self.nodes[index].node.focusable && self.effective_enabled(index)
                    })
                    .collect();
                if order.is_empty() {
                    return;
                }
                let position = self
                    .focused
                    .and_then(|focused| order.iter().position(|&index| index == focused));
                let next = match (position, key) {
                    (Some(position), UiNavKey::Tab) => order[(position + 1) % order.len()],
                    (Some(position), UiNavKey::ShiftTab) => {
                        order[(position + order.len() - 1) % order.len()]
                    }
                    (None, UiNavKey::ShiftTab) => *order.last().unwrap(),
                    (None, _) => order[0],
                    (Some(_), _) => return,
                };
                self.set_focus(Some(next), out);
                self.refresh_states(out);
            }
            UiNavKey::Enter => {
                if let Some(focused) = self.focused {
                    self.dispatch(focused, UiEventType::Click, out);
                    self.dispatch(focused, UiEventType::Submit, out);
                }
            }
            UiNavKey::Escape => {
                self.set_focus(None, out);
                self.refresh_states(out);
            }
        }
    }

    /// Settles rubber-band overshoot after a drag/wheel gesture. Returns whether
    /// anything moved, so hosts can skip redraws on steady frames.
    pub fn tick(&mut self, delta_seconds: f32) -> bool {
        if !delta_seconds.is_finite() || delta_seconds <= 0.0 {
            return false;
        }
        let factor = (delta_seconds * 12.0).clamp(0.0, 1.0);
        let mut moved = false;
        for index in 0..self.nodes.len() {
            let Some((offset, max)) = self.nodes[index].node.scroll.as_ref().map(|scroll| {
                (
                    scroll.offset,
                    scroll.max_offset(self.nodes[index].node.rect),
                )
            }) else {
                continue;
            };
            let clamped = offset.clamp(0.0, max);
            if (offset - clamped).abs() < 0.01 {
                if offset != clamped
                    && let Some(scroll) = &mut self.nodes[index].node.scroll
                {
                    scroll.offset = clamped;
                    moved = true;
                }
                continue;
            }
            if let Some(scroll) = &mut self.nodes[index].node.scroll {
                scroll.offset = offset + (clamped - offset) * factor;
                moved = true;
            }
        }
        moved
    }

    /// Clears transient pointer state on focus loss / scene switch. Focus and
    /// scroll positions survive unless `full` is set.
    pub fn reset(&mut self, full: bool) {
        self.hovered = None;
        self.pressed = None;
        self.drag = None;
        if full {
            self.focused = None;
            for state in &mut self.nodes {
                if let Some(scroll) = &mut state.node.scroll {
                    scroll.offset = 0.0;
                }
                state.visual = ButtonVisualState::Normal;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(key: &str, rect: UiRect) -> UiInteractionNode {
        UiInteractionNode::new(key, rect)
    }

    fn tree() -> UiInteraction {
        let mut ui = UiInteraction::new();
        let root = ui
            .push_node(
                None,
                UiInteractionNode::new("root", UiRect::new(0.0, 0.0, 400.0, 300.0)),
            )
            .unwrap();
        let panel = ui
            .push_node(
                Some(root),
                hit("panel", UiRect::new(10.0, 10.0, 200.0, 200.0)),
            )
            .unwrap();
        ui.push_node(
            Some(panel),
            hit("label", UiRect::new(20.0, 20.0, 50.0, 20.0)),
        )
        .unwrap();
        ui.push_node(
            Some(root),
            hit("button", UiRect::new(300.0, 10.0, 80.0, 40.0))
                .focusable(true)
                .states(ButtonStateSet::default()),
        )
        .unwrap();
        ui
    }

    #[test]
    fn ui_key_is_stable_and_duplicate_keys_are_reported() {
        assert_eq!(
            UiKey::from_node_key("shop.buy"),
            UiKey::from_node_key("shop.buy")
        );
        assert_ne!(
            UiKey::from_node_key("shop.buy"),
            UiKey::from_node_key("shop.sell")
        );
        let mut index = UiKeyIndex::default();
        assert!(index.insert("shop.buy").is_ok());
        assert_eq!(
            index.id_for("shop.buy"),
            Some(UiKey::from_node_key("shop.buy"))
        );
        assert!(matches!(
            index.insert("shop.buy"),
            Err(UiInteractionError::DuplicateKey(_))
        ));
        let mut ui = UiInteraction::new();
        ui.push_node(None, hit("dup", UiRect::new(0.0, 0.0, 1.0, 1.0)))
            .unwrap();
        assert!(matches!(
            ui.push_node(None, hit("dup", UiRect::new(0.0, 0.0, 1.0, 1.0))),
            Err(UiInteractionError::DuplicateKey(_))
        ));
    }

    #[test]
    fn hit_test_prefers_topmost_and_ignores_disabled_subtrees() {
        let mut ui = tree();
        assert_eq!(
            ui.node(ui.hit_test([25.0, 25.0]).unwrap()).unwrap().key,
            "label"
        );
        // Parent disabled hides its whole subtree from hit testing; the click
        // falls through to the enabled ancestor instead of being swallowed.
        let panel = ui.index_of("panel").unwrap();
        ui.nodes[panel].node.enabled = false;
        assert_eq!(
            ui.node(ui.hit_test([25.0, 25.0]).unwrap()).unwrap().key,
            "root"
        );
        // A node outside the root rect is still hit if its own rect matches;
        // clipping is opt-in via `clips`.
        assert_eq!(
            ui.node(ui.hit_test([340.0, 20.0]).unwrap()).unwrap().key,
            "button"
        );
    }

    #[test]
    fn click_dispatches_capture_target_bubble_and_disabled_is_silent() {
        let mut ui = tree();
        let mut out = Vec::new();
        ui.pointer_down([25.0, 25.0], &mut out);
        ui.pointer_up([25.0, 25.0], &mut out);
        let phases: Vec<(UiPhase, String)> = out
            .iter()
            .filter_map(|outcome| match outcome {
                UiOutcome::Dispatch(dispatch) if dispatch.event == UiEventType::Click => {
                    Some((dispatch.phase, dispatch.key.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            phases,
            vec![
                (UiPhase::Capture, "root".to_owned()),
                (UiPhase::Capture, "panel".to_owned()),
                (UiPhase::Target, "label".to_owned()),
                (UiPhase::Bubble, "panel".to_owned()),
                (UiPhase::Bubble, "root".to_owned()),
            ]
        );

        let panel = ui.index_of("panel").unwrap();
        ui.nodes[panel].node.enabled = false;
        let mut out = Vec::new();
        ui.pointer_down([25.0, 25.0], &mut out);
        ui.pointer_up([25.0, 25.0], &mut out);
        assert!(!out.iter().any(|outcome| matches!(
            outcome,
            UiOutcome::Dispatch(UiDispatch {
                event: UiEventType::Click,
                key,
                ..
            }) if key == "panel" || key == "label"
        )));
    }

    #[test]
    fn hover_round_trip_drives_button_states() {
        let mut ui = tree();
        let button = ui.index_of("button").unwrap();
        let mut out = Vec::new();
        ui.pointer_move([340.0, 20.0], &mut out);
        assert_eq!(ui.visual_state(button), Some(ButtonVisualState::Hover));
        assert!(out.iter().any(|outcome| matches!(
            outcome,
            UiOutcome::StateChanged {
                state: ButtonVisualState::Hover,
                ..
            }
        )));
        ui.pointer_down([340.0, 20.0], &mut out);
        assert_eq!(ui.visual_state(button), Some(ButtonVisualState::Pressed));
        ui.pointer_up([340.0, 20.0], &mut out);
        assert_eq!(ui.visual_state(button), Some(ButtonVisualState::Hover));
        // Pointer-down focuses a focusable button, so leaving with the pointer
        // shows the focus ring until focus is cleared.
        ui.pointer_move([1.0, 1.0], &mut out);
        assert_eq!(ui.visual_state(button), Some(ButtonVisualState::Focused));
        ui.key(UiNavKey::Escape, &mut out);
        assert_eq!(ui.visual_state(button), Some(ButtonVisualState::Normal));
        assert!(out.iter().any(|outcome| matches!(
            outcome,
            UiOutcome::Dispatch(UiDispatch {
                event: UiEventType::PointerExit,
                ..
            })
        )));
    }

    #[test]
    fn activate_dispatches_click_without_pointer_release() {
        let mut ui = tree();
        let label = ui.index_of("label").unwrap();
        let mut out = Vec::new();
        ui.activate(label, &mut out);
        let phases: Vec<(UiPhase, String)> = out
            .iter()
            .filter_map(|outcome| match outcome {
                UiOutcome::Dispatch(dispatch) if dispatch.event == UiEventType::Click => {
                    Some((dispatch.phase, dispatch.key.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            phases,
            vec![
                (UiPhase::Capture, "root".to_owned()),
                (UiPhase::Capture, "panel".to_owned()),
                (UiPhase::Target, "label".to_owned()),
                (UiPhase::Bubble, "panel".to_owned()),
                (UiPhase::Bubble, "root".to_owned()),
            ]
        );
    }

    #[test]
    fn tab_enter_escape_move_focus_and_submit() {
        let mut ui = tree();
        let mut out = Vec::new();
        ui.key(UiNavKey::Tab, &mut out);
        assert_eq!(ui.focused_key(), Some("button"));
        ui.key(UiNavKey::Enter, &mut out);
        assert!(out.iter().any(|outcome| matches!(
            outcome,
            UiOutcome::Dispatch(UiDispatch {
                event: UiEventType::Submit,
                ..
            })
        )));
        ui.key(UiNavKey::Escape, &mut out);
        assert_eq!(ui.focused_key(), None);
        let button = ui.index_of("button").unwrap();
        assert_eq!(ui.visual_state(button), Some(ButtonVisualState::Normal));
    }

    #[test]
    fn scroll_wheel_clamps_drag_overshoots_and_rebounds() {
        let mut ui = UiInteraction::new();
        let viewport = ui
            .push_node(
                None,
                UiInteractionNode::new("list", UiRect::new(0.0, 0.0, 100.0, 100.0))
                    .scroll(UiScroll::vertical(300.0)),
            )
            .unwrap();
        ui.push_node(
            Some(viewport),
            hit("item", UiRect::new(0.0, 0.0, 100.0, 300.0)),
        )
        .unwrap();
        let item = ui.index_of("item").unwrap();

        let mut out = Vec::new();
        ui.wheel([50.0, 50.0], 30.0, &mut out);
        assert_eq!(ui.scroll_offset("list"), Some(30.0));
        assert_eq!(ui.effective_rect(item).unwrap().y, -30.0);
        // Hit testing uses the scrolled geometry.
        assert_eq!(ui.hit_test([50.0, 10.0]), Some(item));

        ui.wheel([50.0, 50.0], 10_000.0, &mut out);
        assert_eq!(ui.scroll_offset("list"), Some(200.0));

        // Drag downwards past the top: rubber-band, then settle back to 0.
        ui.wheel([50.0, 50.0], -10_000.0, &mut out);
        assert_eq!(ui.scroll_offset("list"), Some(0.0));
        ui.pointer_down([50.0, 50.0], &mut out);
        ui.pointer_move([50.0, 180.0], &mut out);
        let offset = ui.scroll_offset("list").unwrap();
        assert!(offset < 0.0 && offset > -60.0);
        ui.pointer_up([50.0, 180.0], &mut out);
        for _ in 0..120 {
            ui.tick(1.0 / 60.0);
        }
        assert_eq!(ui.scroll_offset("list"), Some(0.0));
    }

    #[test]
    fn clip_rect_blocks_hit_testing_outside_scroll_view() {
        let mut ui = UiInteraction::new();
        let view = ui
            .push_node(
                None,
                UiInteractionNode::new("view", UiRect::new(0.0, 0.0, 100.0, 100.0)).clips(true),
            )
            .unwrap();
        ui.push_node(
            Some(view),
            hit("overflow", UiRect::new(0.0, 120.0, 50.0, 20.0)),
        )
        .unwrap();
        assert_eq!(ui.hit_test([10.0, 125.0]), None);
        assert_eq!(ui.hit_test([10.0, 50.0]), Some(view));
    }

    #[test]
    fn pressing_inside_scrollview_and_dragging_cancels_click() {
        let mut ui = UiInteraction::new();
        let view = ui
            .push_node(
                None,
                UiInteractionNode::new("view", UiRect::new(0.0, 0.0, 100.0, 100.0))
                    .scroll(UiScroll::vertical(300.0)),
            )
            .unwrap();
        ui.push_node(Some(view), hit("row", UiRect::new(0.0, 0.0, 100.0, 300.0)))
            .unwrap();
        let mut out = Vec::new();
        ui.pointer_down([50.0, 50.0], &mut out);
        out.clear();
        ui.pointer_move([50.0, 20.0], &mut out);
        ui.pointer_up([50.0, 20.0], &mut out);
        assert!(!out.iter().any(|outcome| matches!(
            outcome,
            UiOutcome::Dispatch(UiDispatch {
                event: UiEventType::Click,
                ..
            })
        )));
        assert!(ui.scroll_offset("view").unwrap() > 0.0);
    }
}
