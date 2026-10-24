//! docs/60 §4.1：动态 UI 域的能力单元（P1 ⑤-12 从 `rustlua-scripting` 搬来）。
//!
//! UI 的"运行期动态元素"——脚本登记的文本/进度条/面板/图/网格、按名隐藏集、拖拽状态、
//! 视口与参考分辨率——一直是 `ScriptState` 里最大的一块内联状态（7 个 `BTreeMap` + 3 个标量）。
//! 它们的读写规则（面板标题与提示文本的键拼接、网格命中换算、拖拽的起点记录）全在 UI 域，
//! 所以状态单元也跟着绑定一起搬。
//!
//! **窗口形态部分刻意保留**：`viewport` / `reference` / `scene_text_hidden` 在场景切换时不清
//! （前者是窗口属性、后者按名隐藏跨场景仍生效），这与此前脚本层的既有行为逐字一致。

use crate::lua_nav::NavState;
use crate::ui::{DynamicUiImage, UiGrid, UiPanel};
use crate::ui_binding::BindingValue;
use rustlua_lua_api::Handle;
use std::collections::{BTreeMap, BTreeSet};

/// Lua 驱动的一次背包拖拽状态（`ui.drag_begin` / `ui.drag_end`）。
#[derive(Clone, Debug)]
pub struct UiDrag {
    pub(crate) from_id: String,
    pub(crate) from_index: usize,
}

impl UiDrag {
    #[must_use]
    pub fn from_id(&self) -> &str {
        &self.from_id
    }

    #[must_use]
    pub fn from_index(&self) -> usize {
        self.from_index
    }
}

/// 运行期登记的一段屏幕空间文本（宿主逐帧绘制）。
#[derive(Debug, Clone)]
pub struct UiText {
    pub text: String,
    pub visible: bool,
    pub position: [f32; 2],
    /// 像素字号（SDF em 高度，px）。提供时优先于 `scale`；
    /// 换算见 [`crate::ui::ui_text_effective_scale`]。
    pub font_size: Option<f32>,
    /// 字形缩放系数（`1.0` = SDF 字体 em 12px / 位图 5x7 字形 7px）。
    pub scale: f32,
}

/// 屏幕空间填充条（血条之类），宿主逐帧绘制。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiBar {
    pub position: [f32; 2],
    pub size: [f32; 2],
    /// 填充比例，`0..=1`。
    pub ratio: f32,
    pub color: [f32; 4],
}

/// UI 域里"跨帧的脚本可见状态"。
#[derive(Debug, Default)]
pub struct UiState {
    pub(crate) texts: BTreeMap<String, UiText>,
    pub(crate) bars: BTreeMap<String, UiBar>,
    pub(crate) panels: BTreeMap<String, UiPanel>,
    pub(crate) images: BTreeMap<String, DynamicUiImage>,
    pub(crate) grids: BTreeMap<String, UiGrid>,
    /// `ui.scene_text_visible(name, false)` 登记的场景文字实体按名隐藏集。
    /// 语义与 web 兼容层 `__rustlua_scene_text_hidden` 对齐：实体仍在场景里
    /// （照常参与 UiLayout 布局），仅绘制场景文字时跳过该实体。
    pub(crate) scene_text_hidden: BTreeSet<String>,
    /// M76-02：`UiBindings` 的视图模型（`ui.view_model_set` 写入）。键是**点分源路径**
    /// （如 `data.score`），与 `BindingSet::resolve` 的查表口径一致。
    /// 场景切换时清空：绑定源属于当前场景的数据面。
    pub(crate) view_model: BTreeMap<String, BindingValue>,
    pub(crate) drag: Option<UiDrag>,
    pub(crate) viewport: [f32; 2],
    /// Canvas 空间 UI 参考分辨率（设计分辨率），与运行时渲染保持一致，
    /// 供 Lua 侧 `process_ui_clicks` / `grid_hit` 命中测试使用。
    pub(crate) reference: [f32; 2],
    /// M76-07：`ui.virtual_list` / `ui.rich_text` / `ui.text_input` / `ui.nav_stack`
    /// 的内核状态。都属于场景生命周期内的 UI 状态，切场景一并丢弃。
    pub(crate) advanced: crate::lua_advanced::AdvancedState,
}

impl UiState {
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "ui")
    }

    #[must_use]
    pub fn texts(&self) -> &BTreeMap<String, UiText> {
        &self.texts
    }

    pub fn texts_mut(&mut self) -> &mut BTreeMap<String, UiText> {
        &mut self.texts
    }

    #[must_use]
    pub fn bars(&self) -> &BTreeMap<String, UiBar> {
        &self.bars
    }

    pub fn bars_mut(&mut self) -> &mut BTreeMap<String, UiBar> {
        &mut self.bars
    }

    #[must_use]
    pub fn panels(&self) -> &BTreeMap<String, UiPanel> {
        &self.panels
    }

    pub fn panels_mut(&mut self) -> &mut BTreeMap<String, UiPanel> {
        &mut self.panels
    }

    #[must_use]
    pub fn images(&self) -> &BTreeMap<String, DynamicUiImage> {
        &self.images
    }

    pub fn images_mut(&mut self) -> &mut BTreeMap<String, DynamicUiImage> {
        &mut self.images
    }

    #[must_use]
    pub fn grids(&self) -> &BTreeMap<String, UiGrid> {
        &self.grids
    }

    pub fn grids_mut(&mut self) -> &mut BTreeMap<String, UiGrid> {
        &mut self.grids
    }

    #[must_use]
    pub fn scene_text_hidden(&self) -> &BTreeSet<String> {
        &self.scene_text_hidden
    }

    pub fn hide_scene_text(&mut self, name: String) {
        self.scene_text_hidden.insert(name);
    }

    pub fn show_scene_text(&mut self, name: &str) {
        self.scene_text_hidden.remove(name);
    }

    #[must_use]
    pub fn view_model(&self) -> &BTreeMap<String, BindingValue> {
        &self.view_model
    }

    pub fn view_model_mut(&mut self) -> &mut BTreeMap<String, BindingValue> {
        &mut self.view_model
    }

    #[must_use]
    pub fn drag(&self) -> Option<&UiDrag> {
        self.drag.as_ref()
    }

    pub fn set_drag(&mut self, drag: Option<UiDrag>) {
        self.drag = drag;
    }

    #[must_use]
    pub fn viewport(&self) -> [f32; 2] {
        self.viewport
    }

    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.viewport = [width.max(1) as f32, height.max(1) as f32];
    }

    #[must_use]
    pub fn reference(&self) -> [f32; 2] {
        self.reference
    }

    pub fn set_reference(&mut self, reference: [f32; 2]) {
        self.reference = reference;
    }

    /// 场景切换时丢弃动态登记的 UI 元素与交互状态；`viewport`/`reference`（窗口形态）
    /// 与 `scene_text_hidden`（按名隐藏跨场景仍生效）保留。
    pub fn clear_for_scene_transition(&mut self, nav: &Handle<NavState>) {
        self.texts.clear();
        self.bars.clear();
        self.panels.clear();
        self.images.clear();
        self.grids.clear();
        self.view_model.clear();
        self.drag = None;
        self.advanced.clear_for_scene_transition();
        // 焦点环/滚动位置已在 UI 域（P1 ⑤-3），经能力单元清空。
        if let Ok(mut nav) = nav.borrow_mut() {
            nav.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_names_itself_for_reentrancy_errors() {
        let ui = UiState::handle();
        let _guard = ui.borrow_mut().unwrap();
        let message = ui.borrow().unwrap_err().to_string();
        assert!(message.contains("already borrowed"), "{message}");
        assert!(message.contains("ui"), "{message}");
    }

    /// 场景切换清掉动态元素与拖拽，**但保留**窗口形态与按名隐藏集。
    #[test]
    fn scene_transition_clears_elements_but_keeps_window_shape() {
        let ui = UiState::handle();
        let nav = NavState::handle();
        ui.with_mut(|ui| {
            ui.texts_mut().insert(
                "hud".into(),
                UiText {
                    text: "hp".into(),
                    visible: true,
                    position: [0.0, 0.0],
                    font_size: None,
                    scale: 1.0,
                },
            );
            ui.bars_mut().insert(
                "hp".into(),
                UiBar {
                    position: [0.0, 0.0],
                    size: [10.0, 2.0],
                    ratio: 1.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                },
            );
            ui.hide_scene_text("label".into());
            ui.view_model_mut()
                .insert("data.score".into(), BindingValue::Number(3.0));
            ui.set_viewport(800, 600);
            ui.set_reference([1280.0, 720.0]);
            ui.set_drag(Some(UiDrag {
                from_id: "slot".into(),
                from_index: 2,
            }));
            ui.advanced.virtual_lists.insert(
                "bag".into(),
                crate::lua_advanced::VirtualListBinding {
                    list: crate::ui_virtual_list::VirtualList::new(Default::default(), 10, 100.0),
                    pool: Default::default(),
                },
            );
            ui.advanced.inputs.insert(
                "name".into(),
                crate::ui_text_input::TextInputState::with_text("a"),
            );
            ui.advanced.navigation = Some(crate::ui_nav_stack::NavigationStack::new("title"));
        });
        ui.with_mut(|ui| ui.clear_for_scene_transition(&nav));
        ui.with(|ui| {
            assert!(ui.texts().is_empty());
            assert!(ui.bars().is_empty());
            assert!(ui.view_model().is_empty());
            assert!(ui.drag().is_none());
            assert!(ui.advanced.virtual_lists.is_empty());
            assert!(ui.advanced.inputs.is_empty());
            assert!(ui.advanced.navigation.is_none());
            assert!(ui.scene_text_hidden().contains("label"));
            assert_eq!(ui.viewport(), [800.0, 600.0]);
            assert_eq!(ui.reference(), [1280.0, 720.0]);
        });
    }

    /// 视口最小 1×1：零尺寸会让网格命中除零。
    #[test]
    fn viewport_is_never_zero() {
        let ui = UiState::handle();
        ui.with_mut(|ui| ui.set_viewport(0, 0));
        ui.with(|ui| assert_eq!(ui.viewport(), [1.0, 1.0]));
    }
}
