//! M76-02：运行时 UI 交互接线。
//!
//! 纯逻辑树在 `rustlua_ui::ui_interaction`（命中顺序、捕获/冒泡、hover/pressed/focus、
//! 裁剪与滚动）。这里负责三件事：
//!
//! 1. 每步把同一份命中数据（[`UiHitData`]，与点击矩形同源）翻译成交互树；
//! 2. 把本步键鼠输入喂给树，收集事件/状态/滚动结果；
//! 3. 把需要派发的事件按"实体名 + 事件 + 绑定的 Lua 函数"交给脚本层。
//!
//! **默认开启**（M80-01）：hover/pressed、捕获/冒泡、键盘与手柄空间导航、滚轮/拖拽
//! 只有在交互树接管输入后才真正生效；此前默认走旧路径，等于这些能力在生产里全是死代码。
//!
//! click 由**抬起**触发（M80-07）：按下与抬起命中同一节点且未越过拖拽阈值时派发一次，
//! 与 Unity Button 一致；按下后拖出再松手不触发 click。
//!
//! [`RuntimeScene::set_ui_interaction_enabled`] 是显式回滚档：关闭后调度器回到
//! `process_ui_clicks` 旧路径（按下即 click、所有重叠目标都收到 click），逐字节不变。
//! [`RuntimeScene::set_ui_keyboard_nav`] 单独关掉键盘/手柄导航（Enter/方向键不再抢焦点）。

use std::collections::{BTreeMap, BTreeSet};

use rustlua_input::input::{InputSnapshot, KeyCode, MouseButton};
use rustlua_ui::ui_events::{UiEventType, UiEvents};
use rustlua_ui::ui_interaction::{
    UiDispatch as UiInteractionDispatch, UiInteraction, UiInteractionNode, UiNavKey, UiOutcome,
    UiPhase, UiRect as HitRect,
};
use rustlua_ui::ui_states::ButtonVisualState;

use super::RuntimeScene;
use super::mesh_ui::UiHitData;
use super::ui_controls::UiInputCapture;

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
    /// 开关交互路由。默认开启（见模块文档）；关闭后 `ui_click_dispatch` 回到旧路径。
    pub fn set_ui_interaction_enabled(&mut self, enabled: bool) {
        self.ui_interaction_enabled = enabled;
        if !enabled {
            self.ui_interaction = UiInteraction::new();
            self.ui_interaction_signature = None;
            self.ui_bindings.clear();
            self.ui_key_names.clear();
            self.ui_key_slots.clear();
            self.ui_control_drag = None;
            self.ui_visual_states.clear();
            self.ui_state_transitions = Default::default();
        }
    }

    #[must_use]
    pub fn ui_interaction_enabled(&self) -> bool {
        self.ui_interaction_enabled
    }

    /// 键盘/手柄导航开关（Tab、方向键、Enter/confirm、Esc/cancel）。
    ///
    /// 默认开启：只有存在可聚焦节点时才会产生焦点变化，纯展示 UI 不受影响；
    /// 玩法需要独占这些按键时显式关闭。
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
            if let Some(mask) = &item.mask {
                for pad in mask.padding {
                    mix(&mut signature, &pad.to_bits().to_le_bytes());
                }
            }
            if let Some(group) = &item.canvas_group {
                mix(&mut signature, &group.alpha.to_bits().to_le_bytes());
                mix(
                    &mut signature,
                    &[
                        u8::from(group.interactable),
                        u8::from(group.blocks_raycasts),
                    ],
                );
            }
            mix(&mut signature, &[u8::from(item.control)]);
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
        let mut key_slots: BTreeMap<String, u64> = BTreeMap::new();
        for index in order {
            let item = &data[index];
            let mut key = item.name.clone();
            if tree.index_of(&key).is_some() {
                key = format!("{}#{}", item.name, item.slot);
            }
            let focusable = item.control
                || item.events.as_ref().is_some_and(|events| {
                    events.bindings.iter().any(|binding| {
                        matches!(
                            binding.event.unwrap_or(UiEventType::Click),
                            UiEventType::Click | UiEventType::Submit
                        )
                    })
                });
            // 纯结构节点（只有 Mask / CanvasGroup，自身没有事件）不参与命中：
            // 它们的矩形只是给子孙挂裁剪/开关，自己挡住点击会把整块面板变成死区。
            let has_pointer = item.control
                || item.events.is_some()
                || item.button_states.is_some()
                || self.ui_scroll_ranges.contains_key(&item.slot);
            let disabled = item
                .button_states
                .as_ref()
                .is_some_and(|states| states.disabled_state)
                || item
                    .canvas_group
                    .as_ref()
                    .is_some_and(|group| !group.interactable);
            let mut node = UiInteractionNode::new(
                key.clone(),
                HitRect::new(
                    item.tree_rect.0[0],
                    item.tree_rect.0[1],
                    item.tree_rect.1[0],
                    item.tree_rect.1[1],
                ),
            )
            .focusable(focusable)
            .pointer_events(has_pointer)
            .disabled(disabled);
            if let Some(states) = &item.button_states {
                node = node.states(states.clone());
            }
            // M80-02：`CanvasGroup.blocks_raycasts=false` 让整棵子树穿透命中。
            if let Some(group) = &item.canvas_group {
                node = node.blocks_raycasts(group.blocks_raycasts);
            }
            // M80-02：`Mask` 与滚动容器一样裁剪子孙命中（自身不受自己裁剪）。
            if item.mask.is_some() {
                node = node.clips(true);
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
                key_names.insert(key.clone(), item.name.clone());
                key_slots.insert(key, item.slot);
            }
        }
        self.ui_interaction = tree;
        self.ui_bindings = bindings;
        self.ui_key_names = key_names;
        self.ui_key_slots = key_slots;
        self.ui_interaction_signature = Some(signature);
        // 实体换了、名字复用了，上一个的过渡尾巴不能带过去。
        let alive: BTreeSet<String> = self.ui_key_names.keys().cloned().collect();
        self.ui_state_transitions.retain(|key| alive.contains(key));
    }

    /// 把本步输入喂给交互树并返回待派发事件。
    ///
    /// `capture` 由控件路由先一步给出：焦点在滑块/滚动条上时，方向键要改值而不是
    /// 移动焦点（Unity 的同一行为）。
    ///
    /// click 在**抬起**时由交互树派发（按下/抬起同一命中且未拖拽）；`pointer_down`
    /// 只负责按下去态与捕获，避免按下即触发让"按下拖走再松手"也当成点击。
    pub(super) fn route_ui_input(
        &mut self,
        input: &InputSnapshot,
        delta_seconds: f32,
        capture: UiInputCapture,
    ) -> Vec<UiEventDispatch> {
        if !self.ui_interaction_enabled {
            return Vec::new();
        }
        let cursor = input.cursor_position();
        let point = [cursor.0 as f32, cursor.1 as f32];
        let mut outcomes: Vec<UiOutcome> = Vec::new();
        self.ui_interaction.pointer_move(point, &mut outcomes);
        if input.mouse_just_pressed(MouseButton::Left) {
            self.ui_interaction.pointer_down(point, &mut outcomes);
        }
        if input.mouse_just_released(MouseButton::Left) {
            self.ui_interaction.pointer_up(point, &mut outcomes);
        }
        let (_, scroll_y) = input.scroll_delta();
        if scroll_y != 0.0 {
            // wheel 的 delta 是"内容偏移增量"，与滚轮读数方向相反；一行按 40px 计。
            self.ui_interaction
                .wheel(point, -scroll_y * 40.0, &mut outcomes);
        }
        if self.ui_keyboard_nav {
            let actions = self.input.action_map();
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
            // 方向键/DPad/左摇杆走命名动作：同一份映射既给玩法读，也给焦点导航。
            // 只有存在可聚焦节点时才会有实际效果，纯展示 UI 不受影响。
            // 焦点控件已消费的那个轴跳过导航（改值优先，见 `route_ui_controls`）。
            for (action, nav, captured) in [
                ("ui_left", UiNavKey::Left, capture.left_right),
                ("ui_right", UiNavKey::Right, capture.left_right),
                ("ui_up", UiNavKey::Up, capture.up_down),
                ("ui_down", UiNavKey::Down, capture.up_down),
            ] {
                if !captured && input.action_just_pressed(&actions, action) {
                    self.ui_interaction.key(nav, &mut outcomes);
                }
            }
            // Enter / confirm 与 Esc / cancel 合并成一次判定，避免同一帧重复触发。
            let confirm = input.just_pressed(KeyCode::Enter)
                || input.action_just_pressed(&actions, "confirm");
            if confirm {
                self.ui_interaction.key(UiNavKey::Enter, &mut outcomes);
            }
            let cancel = input.just_pressed(KeyCode::Escape)
                || input.action_just_pressed(&actions, "cancel");
            if cancel {
                self.ui_interaction.key(UiNavKey::Escape, &mut outcomes);
            }
        }
        let _ = self.ui_interaction.tick(delta_seconds);
        self.ui_state_transitions.advance(delta_seconds);

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
                    // M80-07：配了 `transition` 的状态切换先让底色走一段，而不是立刻跳。
                    let previous = self.ui_visual_states.get(&key).copied();
                    let states = self.ui_interaction.states_of(&key).cloned();
                    self.ui_visual_states.insert(key.clone(), state);
                    if let Some(states) = states {
                        self.ui_state_transitions
                            .on_state_changed(&key, &states, previous, state);
                    }
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
