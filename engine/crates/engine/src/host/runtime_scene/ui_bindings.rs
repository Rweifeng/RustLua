//! M76-02：`UiBindings` 的逐帧 resolve 与写回。
//!
//! 纯逻辑在 `rustlua_ui::ui_binding`（解析、`default` 回退、只在变化时发目标、
//! 双向写回）。这里只负责三件宿主侧的事：
//!
//! 1. 每步收集世界里所有 `UiBindings` 组件，合并成一份 [`BindingSet`]；
//! 2. 把视图模型（`ui.view_model_set` 写入）resolve 成目标值，落到实体的
//!    `Text.text`、图像可见性、动态进度条比例上；
//! 3. 双向绑定从目标写回源（[`RuntimeScene::push_ui_binding`]）；
//! 4. `UiAnimation` 轨道由 `AnimationPlayer` 采样后发布成 `anim.<实体>.<属性>`。
//!
//! **落点是有意定死的**：`text.text` 写场景 `Text` 组件；`image.visible` 把该名字的
//! 场景 `Image` 记进隐藏集（绘制与命中同源地跳过它，不改作者颜色）；`progress.value`
//! 写 `ui.bar` 建的同名动态进度条（不存在时给诊断，不凭空造条）。

use std::collections::BTreeSet;

use rustlua_ui::ui::UiText;
use rustlua_ui::ui_binding::{
    BindingError, BindingRuntime, BindingSet, BindingValue, BoundProperty, UiBindings,
};

use super::RuntimeScene;

impl RuntimeScene {
    /// 世界里有没有任何 `UiBindings` 组件（既有场景为 0，走这条路时零 resolve）。
    fn collect_ui_bindings(&mut self) -> Option<BindingSet> {
        let mut bindings = BindingSet::default();
        let mut found = false;
        self.script.with_world(|world| {
            world.for_each::<UiBindings>(|_, bound| {
                found = true;
                bindings.bindings.extend(bound.0.bindings.iter().cloned());
            });
        });
        found.then_some(bindings)
    }

    /// 每步 resolve 一次：只把**变化过**的目标写到 ECS/动态 UI 上。
    ///
    /// 没有绑定时同时清空隐藏集与诊断——它们只对当前这套绑定有意义。
    pub(super) fn resolve_ui_bindings(&mut self) {
        let Some(bindings) = self.collect_ui_bindings() else {
            self.ui_binding_runtime = BindingRuntime::default();
            self.ui_binding_hidden_images.clear();
            self.ui_binding_report.clear();
            return;
        };
        // 绑定被删/目标改名后，旧的隐藏名单必须失效，否则同名图像会一直不可见。
        let image_targets: BTreeSet<String> = bindings
            .bindings
            .iter()
            .filter(|binding| {
                binding.property.unwrap_or(BoundProperty::TextText) == BoundProperty::ImageVisible
            })
            .map(|binding| binding.target.clone())
            .collect();
        self.ui_binding_hidden_images
            .retain(|name| image_targets.contains(name));

        let view_model = self.script.with_ui_view_model(std::clone::Clone::clone);
        let outcomes = self.ui_binding_runtime.update(&bindings, &view_model);
        if outcomes.is_empty() {
            self.ui_binding_report.clear();
            return;
        }

        let mut text_updates: Vec<(String, String)> = Vec::new();
        let mut hidden_updates: Vec<(String, bool)> = Vec::new();
        let mut bar_updates: Vec<(String, f32)> = Vec::new();
        for outcome in outcomes {
            match (outcome.property, outcome.value) {
                (BoundProperty::TextText, BindingValue::Text(text)) => {
                    text_updates.push((outcome.target, text));
                }
                (BoundProperty::ImageVisible, BindingValue::Bool(visible)) => {
                    hidden_updates.push((outcome.target, visible));
                }
                (BoundProperty::ProgressValue, BindingValue::Number(value)) => {
                    bar_updates.push((outcome.target, value.clamp(0.0, 1.0)));
                }
                _ => {}
            }
        }

        let mut report = Vec::new();
        for (name, text) in text_updates {
            let Some(id) = self.script.entity_handle(&name) else {
                report.push(format!("text 绑定目标 {name} 不存在（或已销毁）"));
                continue;
            };
            let applied = self
                .script
                .with_world_mut(|world| match world.get_mut::<UiText>(id) {
                    Ok(Some(component)) => {
                        component.text = text;
                        true
                    }
                    _ => false,
                });
            if !applied {
                report.push(format!("text 绑定目标 {name} 没有 Text 组件"));
            }
        }
        for (name, visible) in hidden_updates {
            if self.script.entity_handle(&name).is_none() {
                report.push(format!("image 绑定目标 {name} 不存在（或已销毁）"));
                continue;
            }
            if visible {
                self.ui_binding_hidden_images.remove(&name);
            } else {
                self.ui_binding_hidden_images.insert(name);
            }
        }
        if !bar_updates.is_empty() {
            let mut missing = Vec::new();
            self.script.with_ui_bars_mut(|bars| {
                for (name, ratio) in bar_updates {
                    match bars.get_mut(&name) {
                        Some(bar) => bar.ratio = ratio,
                        None => missing.push(name),
                    }
                }
            });
            report.extend(
                missing
                    .into_iter()
                    .map(|name| format!("progress 绑定目标 {name} 没有 ui.bar")),
            );
        }
        self.ui_binding_report = report;
    }

    /// 双向绑定：目标侧编辑值写回视图模型的 `source`。
    ///
    /// 单向绑定返回 [`BindingError::NotTwoWay`]（不静默吞掉）；成功后立即 resolve 一次，
    /// 因此同一帧里所有共用该源的绑定都能看到新值。宿主侧（输入框/滑条）与测试共用此入口。
    pub fn push_ui_binding(
        &mut self,
        target: &str,
        property: BoundProperty,
        value: BindingValue,
    ) -> Result<(), BindingError> {
        let Some(bindings) = self.collect_ui_bindings() else {
            return Err(BindingError::BindingNotFound(target.to_owned()));
        };
        // 借用面：交互运行时与 UI 能力单元是两个字段，闭包只捕获前者，不碰 `&mut self`。
        let runtime = &mut self.ui_binding_runtime;
        let result = self.script.with_ui_view_model_mut(|view_model| {
            runtime.push(&bindings, target, property, value, view_model)
        });
        result?;
        self.resolve_ui_bindings();
        Ok(())
    }

    /// 最近一次 resolve 的诊断（目标实体缺失/组件缺失/进度条不存在）。
    #[must_use]
    pub fn ui_binding_report(&self) -> &[String] {
        &self.ui_binding_report
    }

    /// 把 `UiAnimation.<属性>` 轨道（`.anim` 经 `AnimationPlayer` 采样）发布到视图模型
    /// `anim.<实体名>.<属性>`。
    ///
    /// UI 域没有节点属性存储，轨道值统一落在视图模型：既能被 `UiBindings` 当源消费，
    /// 也能被 Lua 读取。每次推进先清掉上一帧的 `anim.` 键，片段停播/实体销毁后不留残值。
    pub(super) fn publish_ui_animation_samples(
        &mut self,
        samples: &[(rustlua_ecs::entity::EntityId, Vec<(String, f32)>)],
    ) {
        let named = samples
            .iter()
            .filter_map(|(id, values)| self.script.entity_name(*id).map(|name| (name, values)))
            .collect::<Vec<_>>();
        self.script.with_ui_view_model_mut(|view_model| {
            view_model.retain(|key, _| !key.starts_with("anim."));
            for (name, values) in &named {
                for (property, value) in values.iter() {
                    view_model.insert(
                        format!("anim.{name}.{property}"),
                        BindingValue::Number(*value),
                    );
                }
            }
        });
    }

    /// `image.visible=false` 的绑定目标当前是否被隐藏（渲染与命中都读它）。
    #[must_use]
    pub fn ui_image_hidden(&self, name: &str) -> bool {
        self.ui_binding_hidden_images.contains(name)
    }
}
