//! Inspector 的纯文本辅助：覆盖行 diff、字段取值、各枚举的显示名。
//!
//! 从 `panels/inspector.rs` 原样搬出（P1 结构收口）。**不碰文档**：输入取值、输出字符串，
//! 因此可以脱离 egui 单独测；`diff_prefab_entity` / `prefab_override_side_value` 仍按原路径
//! （`panels::inspector::…`）由 `mod.rs` 再导出。

use super::*;

// 混合值判定（`has_mixed_transform`/`has_mixed_transform3d`）**已删除**（第 36 轮）：
// `Transform`/`Transform3D` 迁到通用编辑器后，逐字段的 `SchemaFieldRow.mixed` 就是混合值来源
// （按字段而不是按组件判定，比原来更细）。

// Prefab 写路径辅助（`PrefabRebuildState`、`collect_prefab_rebuild_state`、
// `build_prefab_from_instance_root`、`prefab_root_ids`、`collect_prefab_overrides`、
// `prefab_override_value`、`collect_scene_subtree`、`apply_prefab_instance_metadata`、
// `preserve_prefab_editor_flags`、`PrefabWriteRequest`）已整体搬到 shell（`crate::actions`）：
// 它们只在"写回/重新应用/解包"这条提交路径上使用。面板侧只保留**显示用**的
// Prefab diff（`diff_prefab_entity` / `prefab_override_rows`）与源缓存。

/// Prefab 覆盖列表的一行：字段 + 源取值 + 实例取值（都用于显示与选择性回退）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PrefabOverrideRow {
    pub(super) field: String,
    pub(super) source_value: String,
    pub(super) instance_value: String,
}

/// 由覆盖字段串计算某一侧的原始文本（源或实例）。
///
/// `pub(crate)`：M79 P2 的整实例覆盖聚合（`crate::actions::prefab_overrides`）要用同一个口径
/// 生成"源 / 实例"两侧显示，避免出现第二份字段读取规则。
pub(crate) fn prefab_override_side_value(entity: &SceneEntity, field: &str) -> String {
    if field == "name" {
        return entity.name.clone();
    }
    if let Some(key) = field.strip_prefix("component:") {
        return entity
            .components
            .get(key)
            .cloned()
            .unwrap_or_else(|| "（缺失）".to_owned());
    }
    if let Some(raw_path) = field.strip_prefix("property:") {
        let Ok(path) = rustlua_ecs::property_path::PropertyPath::parse(raw_path) else {
            return "（非法路径）".to_owned();
        };
        let Some(payload) = entity.components.get(&path.component) else {
            return "（缺失）".to_owned();
        };
        return rustlua_ecs::property_path::read_field_text(payload, &path)
            .unwrap_or_else(|_| "（缺失）".to_owned());
    }
    if let Some(key) = field.strip_prefix("metadata:") {
        return entity
            .metadata
            .get(key)
            .cloned()
            .unwrap_or_else(|| "（缺失）".to_owned());
    }
    "（未知字段）".to_owned()
}

/// 由 diff 产生的字段串计算「源 / 实例」两侧的原始文本。
pub(super) fn prefab_override_rows(
    source: &SceneEntity,
    instance: &SceneEntity,
) -> Vec<PrefabOverrideRow> {
    diff_prefab_entity(source, instance)
        .into_iter()
        .map(|field| PrefabOverrideRow {
            source_value: prefab_override_side_value(source, &field),
            instance_value: prefab_override_side_value(instance, &field),
            field,
        })
        .collect()
}

/// 列表里只显示前若干字符，避免长组件串把行撑爆。
pub(super) fn compact_value(value: &str) -> String {
    const MAX: usize = 48;
    if value.chars().count() <= MAX {
        return value.to_owned();
    }
    let head: String = value.chars().take(MAX).collect();
    format!("{head}…")
}

pub(super) fn canvas_scale_mode_label(
    mode: rustlua_ui::ui_layout::CanvasScaleMode,
) -> &'static str {
    use rustlua_ui::ui_layout::CanvasScaleMode;
    match mode {
        CanvasScaleMode::None => "不缩放",
        CanvasScaleMode::FitWidth => "适配宽度",
        CanvasScaleMode::FitHeight => "适配高度",
        CanvasScaleMode::FitBoth => "双向适配",
    }
}

pub(super) fn collider3d_shape_label(shape: &str) -> &str {
    match shape {
        "ball" => "球体",
        "cuboid" => "长方体",
        "capsule" => "胶囊",
        _ => shape,
    }
}

pub(super) fn container_kind_label(kind: &str) -> &str {
    match kind {
        "vbox" => "垂直排列",
        "hbox" => "水平排列",
        "grid" => "网格",
        "wrap" => "换行",
        "margin" => "边距",
        "center" => "居中",
        _ => kind,
    }
}

pub(super) fn container_align_label(align: &str) -> &str {
    match align {
        "start" => "起始",
        "center" => "居中",
        "end" => "末尾",
        _ => align,
    }
}

pub(super) fn container_justify_label(justify: &str) -> &str {
    match justify {
        "start" => "起始",
        "center" => "居中",
        "end" => "末尾",
        "space_between" => "两端对齐",
        "space_around" => "环绕分布",
        "space_evenly" => "均匀分布",
        _ => justify,
    }
}

pub(super) fn ui_event_label(event: rustlua_ui::ui_events::UiEventType) -> &'static str {
    use rustlua_ui::ui_events::UiEventType;
    match event {
        UiEventType::PointerEnter => "指针进入",
        UiEventType::PointerExit => "指针离开",
        UiEventType::PointerDown => "指针按下",
        UiEventType::PointerUp => "指针抬起",
        UiEventType::Click => "点击",
        UiEventType::Focus => "聚焦",
        UiEventType::Submit => "提交",
        UiEventType::ValueChanged => "值改变",
        UiEventType::Drag => "拖动",
    }
}

pub(super) fn bound_property_label(
    property: rustlua_ui::ui_binding::BoundProperty,
) -> &'static str {
    use rustlua_ui::ui_binding::BoundProperty;
    match property {
        BoundProperty::TextText => "文本内容",
        BoundProperty::ImageVisible => "图片可见",
        BoundProperty::ProgressValue => "进度值",
    }
}

pub(super) fn ui_easing_label(easing: UiEasing) -> &'static str {
    match easing {
        UiEasing::Linear => "线性",
        UiEasing::Step => "阶跃",
        UiEasing::SmoothStep => "平滑",
    }
}

pub(super) fn ui_animation_property_label(property: &str) -> &str {
    match property {
        "opacity" => "不透明度",
        _ => property,
    }
}

pub(crate) fn diff_prefab_entity(source: &SceneEntity, instance: &SceneEntity) -> Vec<String> {
    let mut overrides = Vec::new();
    if source.name != instance.name {
        overrides.push("name".to_owned());
    }

    let component_keys = source
        .components
        .keys()
        .chain(instance.components.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let registry = rustlua_ecs::component_schema::builtin_registry();
    for key in component_keys {
        let source_value = source.components.get(&key);
        let instance_value = instance.components.get(&key);
        if source_value == instance_value {
            continue;
        }
        // docs/55 阶段 3：**逐字段** Override。两边都在、且该组件有 schema 时，
        // 只把真正变化的字段列为覆盖（其余字段不动），这样编辑一个字段不会冻结整个组件串；
        // 组件新增/删除或没有 schema 时退回组件级覆盖（整串替换）。
        let field_level = match (source_value, instance_value, registry.get(key.as_str())) {
            (Some(source_payload), Some(instance_payload), Some(schema)) => {
                let (Ok(source_data), Ok(instance_data)) =
                    (schema.parse(source_payload), schema.parse(instance_payload))
                else {
                    overrides.push(format!("component:{key}"));
                    continue;
                };
                let mut fields = std::collections::BTreeSet::new();
                for (field, source_field) in &source_data {
                    match instance_data.get(field) {
                        // 比较的是 `FieldValue`（解析后的类型值），不是文本：`0.600` 与
                        // `0.600000` 是同一个值。逐字段还原经 `write_builtin` 重排位数，
                        // 按文本比会把刚还原好的字段永远列成"覆盖"。
                        Some(instance_field) if instance_field == source_field => {}
                        Some(_) => {
                            fields.insert(field.clone());
                        }
                        None => {
                            fields.insert(field.clone());
                        }
                    }
                }
                for field in instance_data.keys() {
                    if !source_data.contains_key(field) {
                        fields.insert(field.clone());
                    }
                }
                // 字段级集合可能为空（例如仅格式不同）：此时不产生噪音覆盖。
                for field in fields {
                    overrides.push(format!("property:{key}.{field}"));
                }
                true
            }
            _ => false,
        };
        if !field_level && source_value != instance_value {
            overrides.push(format!("component:{key}"));
        }
    }

    let metadata_keys = source
        .metadata
        .keys()
        .filter(|key| !key.starts_with("prefab."))
        .chain(
            instance
                .metadata
                .keys()
                .filter(|key| !key.starts_with("prefab.")),
        )
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    for key in metadata_keys {
        if source.metadata.get(&key) != instance.metadata.get(&key) {
            overrides.push(format!("metadata:{key}"));
        }
    }

    overrides
}
