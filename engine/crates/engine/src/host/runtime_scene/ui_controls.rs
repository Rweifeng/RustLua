//! M80-04：运行时值控件的宿主接线（`Toggle` / `Slider` / `Progress` / `Scrollbar`）。
//!
//! 纯模型在 `rustlua_ui`（组件串、方向换算、取值夹取）；这里只做两件事：
//!
//! 1. **布局改写**：按组件当前值把控件子实体（填充条、滑块、勾选图）的矩形改成
//!    应有的位置。改写发生在 `ensure_scene_ui_layout` 的滚动平移之前，因此控件放在
//!    滚动列表里时位移照常叠加。
//! 2. **输入路由**：指针按下/拖拽改值，焦点在控件上时方向键改值、Enter/Space 切 Toggle。
//!    取值换算与布局共用 [`ControlDirection`]，"点在哪 = 值多少 = 填充多长"不会分叉。
//!
//! 改动只写组件值、不写布局缓存：下一帧布局重新解算时自然读到新值。`ValueChanged`
//! 事件按实体 `UiEvents` 绑定派发，与 click 走同一条 `process_ui_event` 通道。

use std::collections::BTreeMap;

use rustlua_ecs::entity::EntityId;
use rustlua_input::input::{InputSnapshot, KeyCode, MouseButton};
use rustlua_ui::ui_control_direction::ControlDirection;
use rustlua_ui::ui_events::UiEventType;
use rustlua_ui::ui_layout::UiRect;
use rustlua_ui::ui_progress::Progress;
use rustlua_ui::ui_scene_layout::ResolvedUiNode;
use rustlua_ui::ui_scrollbar::ScrollBar;
use rustlua_ui::ui_slider::Slider;
use rustlua_ui::ui_toggle::{Toggle, set_toggle_on};

use super::RuntimeScene;
use super::ui_input::UiEventDispatch;

/// 控件已消费的导航轴：交互树不再用这个轴移动焦点。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct UiInputCapture {
    pub left_right: bool,
    pub up_down: bool,
}

/// 键盘方向键单步（Unity `Slider` 的 10% 行程）。
const KEYBOARD_STEP_FRACTION: f32 = 0.1;
/// `Scrollbar` 的键盘步长（Unity 的 10% 滚动量）。
const SCROLLBAR_KEYBOARD_STEP: f32 = 0.1;

#[derive(Clone, Debug)]
enum ControlSpec {
    Toggle(Toggle),
    Slider(Slider),
    Progress(Progress),
    ScrollBar(ScrollBar),
}

#[derive(Clone, Debug)]
struct ControlSnapshot {
    id: EntityId,
    spec: ControlSpec,
}

impl RuntimeScene {
    /// 按组件当前值改写控件子实体的布局矩形（内容空间）。
    pub(super) fn apply_control_layout(&self, resolved: &mut BTreeMap<u64, ResolvedUiNode>) {
        let snapshot = self.control_snapshot();
        if snapshot.is_empty() {
            return;
        }
        let names = self.script.entity_names_by_slot();
        let slot_by_name: BTreeMap<&str, u64> = names
            .iter()
            .map(|(slot, name)| (name.as_str(), *slot))
            .collect();

        let mut overrides: Vec<(u64, UiRect)> = Vec::new();
        for (&slot, control) in &snapshot {
            let Some(control_rect) = resolved.get(&slot).map(|node| node.rect) else {
                continue;
            };
            // 目标只认控件的布局子树：同名实体在别处时不得被改写。
            let target = |name: &str| -> Option<(u64, UiRect)> {
                if name.is_empty() {
                    return None;
                }
                let target_slot = *slot_by_name.get(name)?;
                if target_slot == slot || !self.is_layout_descendant(target_slot, slot) {
                    return None;
                }
                resolved
                    .get(&target_slot)
                    .map(|node| (target_slot, node.rect))
            };
            match &control.spec {
                ControlSpec::Toggle(toggle) => {
                    if let Some((target_slot, _)) = target(&toggle.checkmark) {
                        overrides.push((target_slot, toggle.checkmark_rect(control_rect)));
                    }
                }
                ControlSpec::Slider(slider) => {
                    if let Some((target_slot, _)) = target(&slider.fill) {
                        overrides.push((target_slot, slider.fill_rect(control_rect)));
                    }
                    if let Some((target_slot, current)) = target(&slider.handle) {
                        // 滑块保留作者尺寸，只换位置；尺寸仍读作者布局那一份。
                        let size = [current.w, current.h];
                        overrides.push((target_slot, slider.handle_rect(control_rect, size)));
                    }
                }
                ControlSpec::Progress(progress) => {
                    if let Some((target_slot, _)) = target(&progress.fill) {
                        overrides.push((target_slot, progress.fill_rect(control_rect)));
                    }
                }
                ControlSpec::ScrollBar(scrollbar) => {
                    if let Some((target_slot, _)) = target(&scrollbar.handle) {
                        overrides.push((target_slot, scrollbar.handle_rect(control_rect)));
                    }
                }
            }
        }
        for (target_slot, rect) in overrides {
            if let Some(node) = resolved.get_mut(&target_slot) {
                node.rect = rect;
                node.container_controlled = true;
            }
        }
    }

    /// 指针与键盘的控件路由。返回本步消费的导航轴与待派发事件。
    ///
    /// 必须在 `sync_ui_interaction` 之后、`route_ui_input` 之前调用：前者提供命中树，
    /// 后者按 `capture` 让出已消费的方向键。
    pub(super) fn route_ui_controls(
        &mut self,
        input: &InputSnapshot,
        _delta_seconds: f32,
    ) -> (UiInputCapture, Vec<UiEventDispatch>) {
        let mut capture = UiInputCapture::default();
        let mut dispatches = Vec::new();
        if !self.ui_interaction_enabled {
            self.ui_control_drag = None;
            return (capture, dispatches);
        }
        let snapshot = self.control_snapshot();
        if snapshot.is_empty() {
            self.ui_control_drag = None;
            return (capture, dispatches);
        }
        let cursor = input.cursor_position();
        let point = [cursor.0 as f32, cursor.1 as f32];

        // 拖拽中：不看命中树（指针可以拖出控件），只看左键是否还按着。
        if let Some(key) = self.ui_control_drag.clone() {
            let slot = self.ui_key_slots.get(&key).copied();
            match slot.and_then(|slot| snapshot.get(&slot).map(|control| (slot, control))) {
                Some((slot, control)) if input.mouse_is_down(MouseButton::Left) => {
                    if let ControlSpec::Slider(slider) = &control.spec {
                        if let Some(rect) = self.control_rect(slot) {
                            let fraction = slider.direction.fraction(rect, point);
                            if self.write_slider(control.id, Some(fraction), None) {
                                self.push_value_changed(&mut dispatches, slot);
                            }
                        }
                    } else if let ControlSpec::ScrollBar(scrollbar) = &control.spec
                        && let Some(rect) = self.control_rect(slot)
                    {
                        let fraction = scrollbar.direction.fraction(rect, point);
                        if self.write_scrollbar_value(control.id, fraction) {
                            self.push_value_changed(&mut dispatches, slot);
                        }
                    }
                }
                _ => self.ui_control_drag = None,
            }
        }

        if input.mouse_just_pressed(MouseButton::Left)
            && let Some(slot) = self.control_slot_at(point, &snapshot)
            && let Some(control) = snapshot.get(&slot)
        {
            match &control.spec {
                ControlSpec::Toggle(toggle) => {
                    let changed = self
                        .script
                        .with_world_mut(|world| set_toggle_on(world, control.id, !toggle.is_on));
                    for id in changed {
                        self.push_value_changed(&mut dispatches, id.index() as u64);
                    }
                }
                ControlSpec::Slider(slider) => {
                    if let Some(rect) = self.control_rect(slot) {
                        let fraction = slider.direction.fraction(rect, point);
                        self.ui_control_drag = self.interaction_key(slot).map(str::to_owned);
                        if self.write_slider(control.id, Some(fraction), None) {
                            self.push_value_changed(&mut dispatches, slot);
                        }
                    }
                }
                ControlSpec::ScrollBar(scrollbar) => {
                    if let Some(rect) = self.control_rect(slot) {
                        let fraction = scrollbar.direction.fraction(rect, point);
                        self.ui_control_drag = self.interaction_key(slot).map(str::to_owned);
                        if self.write_scrollbar_value(control.id, fraction) {
                            self.push_value_changed(&mut dispatches, slot);
                        }
                    }
                }
                ControlSpec::Progress(_) => {}
            }
        }

        if self.ui_keyboard_nav
            && let Some(focused) = self.ui_focused.clone()
            && let Some(&slot) = self.ui_key_slots.get(&focused)
            && let Some(control) = snapshot.get(&slot)
        {
            let actions = self.input.action_map();
            match &control.spec {
                ControlSpec::Toggle(toggle) => {
                    let confirm = input.just_pressed(KeyCode::Enter)
                        || input.just_pressed(KeyCode::Space)
                        || input.action_just_pressed(&actions, "confirm");
                    if confirm {
                        let changed = self.script.with_world_mut(|world| {
                            set_toggle_on(world, control.id, !toggle.is_on)
                        });
                        for id in changed {
                            self.push_value_changed(&mut dispatches, id.index() as u64);
                        }
                    }
                }
                ControlSpec::Slider(slider) => {
                    capture.left_right |= !slider.direction.is_vertical();
                    capture.up_down |= slider.direction.is_vertical();
                    let (negative, positive) = direction_actions(slider.direction);
                    let delta = key_axis_delta(input, &actions, negative, positive);
                    if delta != 0.0 {
                        let step = if slider.whole_numbers {
                            1.0
                        } else {
                            (slider.max - slider.min) * KEYBOARD_STEP_FRACTION
                        };
                        if self.write_slider(control.id, None, Some(slider.value + delta * step)) {
                            self.push_value_changed(&mut dispatches, slot);
                        }
                    }
                }
                ControlSpec::ScrollBar(scrollbar) => {
                    capture.left_right |= !scrollbar.direction.is_vertical();
                    capture.up_down |= scrollbar.direction.is_vertical();
                    let (negative, positive) = direction_actions(scrollbar.direction);
                    let delta = key_axis_delta(input, &actions, negative, positive);
                    if delta != 0.0
                        && self.write_scrollbar_value(
                            control.id,
                            scrollbar.value + delta * SCROLLBAR_KEYBOARD_STEP,
                        )
                    {
                        self.push_value_changed(&mut dispatches, slot);
                    }
                }
                ControlSpec::Progress(_) => {}
            }
        }

        (capture, dispatches)
    }

    fn control_snapshot(&self) -> BTreeMap<u64, ControlSnapshot> {
        self.script.with_world(|world| {
            let mut controls = BTreeMap::new();
            world.for_each::<Toggle>(|id, toggle| {
                controls.insert(
                    id.index() as u64,
                    ControlSnapshot {
                        id,
                        spec: ControlSpec::Toggle(toggle.clone()),
                    },
                );
            });
            world.for_each::<Slider>(|id, slider| {
                controls.insert(
                    id.index() as u64,
                    ControlSnapshot {
                        id,
                        spec: ControlSpec::Slider(slider.clone()),
                    },
                );
            });
            world.for_each::<Progress>(|id, progress| {
                controls.insert(
                    id.index() as u64,
                    ControlSnapshot {
                        id,
                        spec: ControlSpec::Progress(progress.clone()),
                    },
                );
            });
            world.for_each::<ScrollBar>(|id, scrollbar| {
                controls.insert(
                    id.index() as u64,
                    ControlSnapshot {
                        id,
                        spec: ControlSpec::ScrollBar(scrollbar.clone()),
                    },
                );
            });
            controls
        })
    }

    fn control_rect(&self, slot: u64) -> Option<UiRect> {
        self.ui_layout_rects.get(&slot).map(|node| node.rect)
    }

    /// 命中点下最近的控件（含控件内带事件的图形子节点，沿父链向上找）。
    fn control_slot_at(
        &self,
        point: [f32; 2],
        snapshot: &BTreeMap<u64, ControlSnapshot>,
    ) -> Option<u64> {
        let index = self.ui_interaction.hit_test(point)?;
        let path = self.ui_interaction.path_keys(index);
        for key in path.iter().rev() {
            let Some(&slot) = self.ui_key_slots.get(*key) else {
                continue;
            };
            if snapshot.contains_key(&slot) {
                return Some(slot);
            }
        }
        None
    }

    fn interaction_key(&self, slot: u64) -> Option<&str> {
        self.ui_key_slots
            .iter()
            .find_map(|(key, &candidate)| (candidate == slot).then_some(key.as_str()))
    }

    fn push_value_changed(&self, dispatches: &mut Vec<UiEventDispatch>, slot: u64) {
        let Some(key) = self.interaction_key(slot) else {
            return;
        };
        let Some(name) = self.ui_key_names.get(key).cloned() else {
            return;
        };
        let functions = self
            .ui_bindings
            .get(key)
            .map(|events| events.functions_for(UiEventType::ValueChanged))
            .unwrap_or_default();
        if functions.is_empty() {
            return;
        }
        dispatches.push(UiEventDispatch {
            name,
            event: UiEventType::ValueChanged,
            functions,
        });
    }

    /// `fraction` 与 `value` 二选一：拖拽按比例、键盘按值。
    fn write_slider(&mut self, id: EntityId, fraction: Option<f32>, value: Option<f32>) -> bool {
        self.script.with_world_mut(|world| {
            world
                .get_mut::<Slider>(id)
                .ok()
                .flatten()
                .is_some_and(|slider| match (fraction, value) {
                    (Some(fraction), _) => slider.set_normalized(fraction),
                    (None, Some(value)) => slider.set_value(value),
                    (None, None) => false,
                })
        })
    }

    fn write_scrollbar_value(&mut self, id: EntityId, value: f32) -> bool {
        self.script.with_world_mut(|world| {
            world
                .get_mut::<ScrollBar>(id)
                .ok()
                .flatten()
                .is_some_and(|scrollbar| scrollbar.set_value(value))
        })
    }
}

/// 键盘动作按方向的符号：`(减小的动作, 增大的动作)`。
fn direction_actions(direction: ControlDirection) -> (&'static str, &'static str) {
    match direction {
        ControlDirection::LeftToRight => ("ui_left", "ui_right"),
        ControlDirection::RightToLeft => ("ui_right", "ui_left"),
        ControlDirection::BottomToTop => ("ui_down", "ui_up"),
        ControlDirection::TopToBottom => ("ui_up", "ui_down"),
    }
}

fn key_axis_delta(
    input: &InputSnapshot,
    actions: &rustlua_input::input::ActionMap,
    negative: &str,
    positive: &str,
) -> f32 {
    if input.action_just_pressed(actions, positive) {
        1.0
    } else if input.action_just_pressed(actions, negative) {
        -1.0
    } else {
        0.0
    }
}
