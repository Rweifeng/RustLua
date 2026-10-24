//! M76-02：运行时 UI 交互接线。
//!
//! 纯逻辑树在 `rustlua_ui::ui_interaction`（命中顺序、捕获/冒泡、hover/pressed/focus、
//! 裁剪与滚动）。这里负责三件事：
//!
//! 1. 每步把同一份命中数据（[`UiHitData`]，与点击矩形同源）翻译成交互树；
//! 2. 把本步键鼠输入喂给树，收集事件/状态/滚动结果；
//! 3. 把需要派发的事件按"实体名 + 事件 + 绑定的 Lua 函数"交给脚本层。
//!
//! **默认关闭**：既有场景的契约是"按下即 click、只分发 click、`ButtonStates` 只给编辑器"。
//! 交互路由改变 click 的命中语义（取最上层而不是所有重叠目标）并会派发
//! pointer_enter/exit/down/up，因此由宿主显式 [`RuntimeScene::set_ui_interaction_enabled`]
//! 打开；关闭时调度器仍走旧的 `process_ui_clicks` 路径，逐字节不变。

use std::collections::BTreeMap;

use rustlua_input::input::{InputSnapshot, KeyCode, MouseButton};
use rustlua_ui::ui_events::{UiEventType, UiEvents};
use rustlua_ui::ui_interaction::{
    UiDispatch as UiInteractionDispatch, UiInteraction, UiInteractionNode, UiNavKey, UiOutcome,
    UiPhase, UiRect as HitRect,
};
use rustlua_ui::ui_states::ButtonVisualState;

use super::RuntimeScene;
use super::mesh_ui::UiHitData;

/// 一次待派发的 UI 事件：目标实体名 + 结构化事件 + 该实体在这个事件上绑定的函数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UiEventDispatch {
    pub name: String,
    pub event: UiEventType,
    pub functions: Vec<String>,
}

fn mix(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

impl RuntimeScene {
    /// 开关交互路由。默认关闭（见模块文档）；打开后 `ui_click_dispatch` 改用交互树。
    pub fn set_ui_interaction_enabled(&mut self, enabled: bool) {
        self.ui_interaction_enabled = enabled;
        if !enabled {
            self.ui_interaction = UiInteraction::new();
            self.ui_interaction_signature = None;
            self.ui_bindings.clear();
            self.ui_key_names.clear();
            self.ui_visual_states.clear();
        }
    }

    #[must_use]
    pub fn ui_interaction_enabled(&self) -> bool {
        self.ui_interaction_enabled
    }

    /// 键盘导航（Tab/Shift+Tab/Enter/Esc）也是开关：默认关闭，避免 Tab/Enter 在
    /// 玩法脚本里被 UI 焦点截胡。
    pub fn set_ui_keyboard_nav(&mut self, enabled: bool) {
        self.ui_keyboard_nav = enabled;
    }

    #[must_use]
    pub fn ui_visual_state(&self, name: &str) -> Option<ButtonVisualState> {
        self.ui_visual_states.get(name).copied()
    }

    #[must_use]
    pub fn ui_focused_name(&self) -> Option<&str> {
        self.ui_focused.as_deref()
    }

    #[must_use]
    pub fn ui_scroll_offset(&self, name: &str) -> Option<f32> {
        self.ui_scroll_offsets.get(name).copied()
    }

    /// 按命中数据重建交互树。数据没变时一次分配都不做（修订号/签名比对在前，
    /// 克隆在后）。
    pub(super) fn sync_ui_interaction(&mut self, data: &[UiHitData]) {
        let mut signature = 0xcbf2_9ce4_8422_2325_u64;
        for item in data {
            mix(&mut signature, &item.slot.to_le_bytes());
            mix(&mut signature, item.name.as_bytes());
            for value in [
                item.tree_rect.0[0],
                item.tree_rect.0[1],
                item.tree_rect.1[0],
                item.tree_rect.1[1],
            ] {
                mix(&mut signature, &value.to_bits().to_le_bytes());
            }
            if let Some(events) = &item.events {
                mix(&mut signature, events.to_component_str().as_bytes());
            }
            if let Some(states) = &item.button_states {
                mix(&mut signature, states.to_component_str().as_bytes());
            }
            if let Some(range) = self.ui_scroll_ranges.get(&item.slot) {
                mix(&mut signature, &range.content.to_bits().to_le_bytes());
                mix(&mut signature, &[u8::from(range.horizontal)]);
            }
        }
        if self.ui_interaction_signature == Some(signature) {
            return;
        }

        // 父链只认"同样可交互"的祖先：容器若没有事件/状态就不必进树，否则每帧
        // 都要为纯视觉容器建节点，且它们会把命中吞掉。
        let slots: Vec<u64> = data.iter().map(|item| item.slot).collect();
        let mut parent_of: BTreeMap<u64, Option<u64>> = BTreeMap::new();
        for item in data {
            let mut parent = self
                .ui_scene_nodes
                .get(&item.slot)
                .and_then(|node| node.parent);
            let mut resolved = None;
            while let Some(slot) = parent {
                if slots.contains(&slot) {
                    resolved = Some(slot);
                    break;
                }
                parent = self.ui_scene_nodes.get(&slot).and_then(|node| node.parent);
            }
            parent_of.insert(item.slot, resolved);
        }
        let mut order: Vec<usize> = (0..data.len()).collect();
        let depth = |slot: u64| {
            let mut depth = 0usize;
            let mut current = parent_of.get(&slot).copied().flatten();
            while let Some(slot) = current {
                depth += 1;
                current = parent_of.get(&slot).copied().flatten();
            }
            depth
        };
        order.sort_by_key(|&index| depth(data[index].slot));

        let mut tree = UiInteraction::new();
        let mut index_by_slot: BTreeMap<u64, usize> = BTreeMap::new();
        let mut bindings: BTreeMap<String, UiEvents> = BTreeMap::new();
        let mut key_names: BTreeMap<String, String> = BTreeMap::new();
        for index in order {
            let item = &data[index];
            let mut key = item.name.clone();
            if tree.index_of(&key).is_some() {
                key = format!("{}#{}", item.name, item.slot);
            }
            let focusable = item.events.as_ref().is_some_and(|events| {
                events.bindings.iter().any(|binding| {
                    matches!(
                        binding.event.unwrap_or(UiEventType::Click),
                        UiEventType::Click | UiEventType::Submit
                    )
                })
            });
            let mut node = UiInteractionNode::new(
                key.clone(),
                HitRect::new(
                    item.tree_rect.0[0],
                    item.tree_rect.0[1],
                    item.tree_rect.1[0],
                    item.tree_rect.1[1],
                ),
            )
            .focusable(focusable);
            if let Some(states) = &item.button_states {
                node = node.disabled(states.disabled_state).states(states.clone());
            }
            // M76-02：滚动容器节点带范围并裁剪子节点命中。
            if let Some(range) = self.ui_scroll_ranges.get(&item.slot) {
                node = node.scroll(*range);
            }
            let parent = parent_of
                .get(&item.slot)
                .copied()
                .flatten()
                .and_then(|slot| index_by_slot.get(&slot).copied());
            if let Ok(node_index) = tree.push_node(parent, node) {
                index_by_slot.insert(item.slot, node_index);
                if let Some(events) = &item.events {
                    bindings.insert(key.clone(), events.clone());
                }
                key_names.insert(key, item.name.clone());
            }
        }
        self.ui_interaction = tree;
        self.ui_bindings = bindings;
        self.ui_key_names = key_names;
        self.ui_interaction_signature = Some(signature);
    }

    /// 把本步输入喂给交互树并返回待派发事件。
    ///
    /// click 沿用引擎既有契约：**按下即 click**（`pointer_down` 后立刻激活命中节点），
    /// 抬起阶段产生的 click 被丢弃，避免同一次点击派发两遍。
    pub(super) fn route_ui_input(
        &mut self,
        input: &InputSnapshot,
        delta_seconds: f32,
    ) -> Vec<UiEventDispatch> {
        if !self.ui_interaction_enabled {
            return Vec::new();
        }
        let cursor = input.cursor_position();
        let point = [cursor.0 as f32, cursor.1 as f32];
        let mut outcomes: Vec<UiOutcome> = Vec::new();
        self.ui_interaction.pointer_move(point, &mut outcomes);
        if input.mouse_just_pressed(MouseButton::Left) {
            let hit = self.ui_interaction.hit_test(point);
            self.ui_interaction.pointer_down(point, &mut outcomes);
            if let Some(hit) = hit {
                self.ui_interaction.activate(hit, &mut outcomes);
            }
        }
        if input.mouse_just_released(MouseButton::Left) {
            let before = outcomes.len();
            self.ui_interaction.pointer_up(point, &mut outcomes);
            let tail = outcomes.split_off(before);
            outcomes.extend(tail.into_iter().filter(|outcome| {
                !matches!(outcome, UiOutcome::Dispatch(dispatch) if dispatch.event == UiEventType::Click)
            }));
        }
        let (_, scroll_y) = input.scroll_delta();
        if scroll_y != 0.0 {
            // wheel 的 delta 是"内容偏移增量"，与滚轮读数方向相反；一行按 40px 计。
            self.ui_interaction
                .wheel(point, -scroll_y * 40.0, &mut outcomes);
        }
        if self.ui_keyboard_nav {
            if input.just_pressed(KeyCode::Tab) {
                let shift = input.is_down(KeyCode::ShiftLeft) || input.is_down(KeyCode::ShiftRight);
                self.ui_interaction.key(
                    if shift {
                        UiNavKey::ShiftTab
                    } else {
                        UiNavKey::Tab
                    },
                    &mut outcomes,
                );
            }
            if input.just_pressed(KeyCode::Enter) {
                self.ui_interaction.key(UiNavKey::Enter, &mut outcomes);
            }
            if input.just_pressed(KeyCode::Escape) {
                self.ui_interaction.key(UiNavKey::Escape, &mut outcomes);
            }
        }
        let _ = self.ui_interaction.tick(delta_seconds);

        let mut dispatch_keys: Vec<(String, UiEventType)> = Vec::new();
        for outcome in outcomes {
            match outcome {
                UiOutcome::Dispatch(UiInteractionDispatch { phase, key, event }) => {
                    if !matches!(phase, UiPhase::Target | UiPhase::Bubble) {
                        continue;
                    }
                    if !dispatch_keys
                        .iter()
                        .any(|(seen_key, seen_event)| *seen_key == key && *seen_event == event)
                    {
                        dispatch_keys.push((key, event));
                    }
                }
                UiOutcome::StateChanged { key, state } => {
                    self.ui_visual_states.insert(key, state);
                }
                UiOutcome::FocusChanged { to, .. } => {
                    self.ui_focused = to;
                }
                UiOutcome::Scrolled { key, offset } => {
                    self.ui_scroll_offsets.insert(key, offset);
                }
            }
        }
        dispatch_keys
            .into_iter()
            .filter_map(|(key, event)| {
                let name = self.ui_key_names.get(&key)?.clone();
                let bindings = self.ui_bindings.get(&key);
                let functions = bindings
                    .map(|events| events.functions_for(event))
                    .unwrap_or_default();
                // click 即使没绑函数也要走：旧路径对每个命中目标都投实体 on_interact。
                if functions.is_empty() && !(event == UiEventType::Click && bindings.is_some()) {
                    return None;
                }
                Some(UiEventDispatch {
                    name,
                    event,
                    functions,
                })
            })
            .collect()
    }
}
