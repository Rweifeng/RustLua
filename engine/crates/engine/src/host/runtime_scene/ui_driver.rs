//! 宿主侧的 UI 读写视图（`docs/62` P1 依赖倒置的落点之一）。
//!
//! UI 的**绑定**住在 `rustlua-ui`，**能力单元**由 [`crate::host::script_packs`] 建，
//! 这里给宿主一份带类型的直连视图：文本/条/视图模型/动态面板的读改、视口与参考分辨率的
//! 登记、场景文字隐藏集的查询。
//!
//! 之所以整段搬到宿主：这些访问器的签名带着 `UiText` / `UiBar` / `BindingValue` 等 UI 域
//! 类型，`rustlua-scripting` 只要还留着它们，`Cargo.toml` 那条边就删不掉。搬过来之后脚本层
//! 对 UI 域零引用，调用方只把 `scene.script.with_ui(..)` 改成 `scene.with_ui(..)`——
//! 读写同一份能力单元，行为一字未动。
//!
//! `RuntimeScene`（或经编辑器预览会话转发），这两个 setter 也一并搬到这里，直写能力单元；
//! `LuaRuntime` 上不再有 UI 侧入口。

use super::RuntimeScene;
use rustlua_ecs::entity::EntityId;
use std::collections::{BTreeMap, BTreeSet};

impl RuntimeScene {
    /// 动态文本与条的只读视图（`ui.text` / `ui.bar` 写入）。
    pub fn with_ui<R>(
        &self,
        f: impl FnOnce(
            &BTreeMap<String, rustlua_ui::ui_state::UiText>,
            &BTreeMap<String, rustlua_ui::ui_state::UiBar>,
        ) -> R,
    ) -> R {
        self.ui_state.with(|ui| f(ui.texts(), ui.bars()))
    }

    /// `UiBindings` 视图模型的只读视图（`ui.view_model_set` 写入）。
    pub fn with_ui_view_model<R>(
        &self,
        f: impl FnOnce(&BTreeMap<String, rustlua_ui::ui_binding::BindingValue>) -> R,
    ) -> R {
        self.ui_state.with(|ui| f(ui.view_model()))
    }

    /// 双向绑定的写回入口：只改视图模型，目标由 [`super::ui_bindings`] 的 resolve 落地。
    pub fn with_ui_view_model_mut<R>(
        &self,
        f: impl FnOnce(&mut BTreeMap<String, rustlua_ui::ui_binding::BindingValue>) -> R,
    ) -> R {
        self.ui_state.with_mut(|ui| f(ui.view_model_mut()))
    }

    /// `progress.value` 绑定的落点：按 id 改动态进度条的填充比例（不创建新条）。
    pub fn with_ui_bars_mut<R>(
        &self,
        f: impl FnOnce(&mut BTreeMap<String, rustlua_ui::ui_state::UiBar>) -> R,
    ) -> R {
        self.ui_state.with_mut(|ui| f(ui.bars_mut()))
    }

    /// 运行期 Lua 创建的面板/图/网格的只读视图。
    pub fn with_dynamic_ui<R>(
        &self,
        f: impl FnOnce(
            &BTreeMap<String, rustlua_ui::ui::UiPanel>,
            &BTreeMap<String, rustlua_ui::ui::DynamicUiImage>,
            &BTreeMap<String, rustlua_ui::ui::UiGrid>,
        ) -> R,
    ) -> R {
        self.ui_state
            .with(|ui| f(ui.panels(), ui.images(), ui.grids()))
    }

    /// 场景文字按名隐藏集（`ui.scene_text_visible` 写入）的只读视图。
    #[must_use]
    pub fn scene_text_hidden(&self) -> BTreeSet<String> {
        self.ui_state.with(|ui| ui.scene_text_hidden().clone())
    }

    /// 一次借用判定"实体的名字是否被隐藏"（任一名字命中即隐藏）。
    #[must_use]
    pub fn is_entity_scene_text_hidden(&self, id: EntityId) -> bool {
        let hidden = self.scene_text_hidden();
        let entities = self.script.entity_state_handle();
        entities.with(|entities| {
            entities
                .names
                .any_name_matches(id, |name| hidden.contains(name))
        })
    }

    /// 当前视口尺寸（Lua `ui.grid_hit` 按它解析屏幕坐标）；渲染每帧前调用。
    pub fn set_ui_viewport(&mut self, width: u32, height: u32) {
        self.ui_state.with_mut(|ui| ui.set_viewport(width, height));
    }
}
