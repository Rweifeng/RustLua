//! M76-①：`.ui.json`（UiSpec）→ 场景 UI 的**确定性导出**（docs/75 §4.4/§13）。
//!
//! 运行时不认识 Spec：三宿主只吃 `.scene` 的 `UiLayout`/`Container`/`LayoutItem`/
//! `Text`/`Image`/`UiEvents` 组件。本模块是 Spec 与这套组件文法之间唯一的桥——
//! 编辑器侧纯函数，同一个 Spec 永远导出同一份场景（键有序、串走 `to_component_str`
//! 的既有真源），所以宿主证据可以对着一份**落盘的导出产物**复现。
//!
//! 刻意是 **v1 子集**：映射不到的形状（stretch 轴带 position/min/max、offsets 再叠
//! size/percent、`center`/`margin` 之外的 v2 节点、状态样式）一律报错而不是退化
//! 导出——No-Dead-Data：宁可让 AI 改契约，不能让宿主证据画出一份"差不多"的 UI。

use std::collections::{BTreeMap, BTreeSet};

use rustlua_scene::scene::{Scene, SceneEntityId};
use rustlua_ui::ui::{UI_TEXT_BASE_PX, UiImage, UiText};
use rustlua_ui::ui_events::UiEventType;
use rustlua_ui::ui_layout::{
    ChildAlign, ContainerKind, ContainerStyle, Justify, LayoutItem, UiRectLayout,
};
use rustlua_ui_spec::{
    Align, AnchorPreset, ContainerDirection, Justify as SpecJustify, UiLayoutSpec, UiNode,
    UiNodeKind, UiSpec,
};
use serde_json::Value;

/// 预览器对"无 size/percent"子项的默认测量值（preview.rs `measure_child`），
/// 烘焙的固有尺寸必须是这个口径，否则导出件与预览各说各话。
const SPEC_DEFAULT_W: f32 = 120.0;
const SPEC_DEFAULT_H: f32 = 32.0;

/// UiSpec → 场景（内存对象）。实体名 = 节点 key（`ui_rects_by_name` 按名取值），
/// 实体 id 按深度优先先序从 1 连续编号 = 绘制顺序。
///
/// 错误是聚合的：一次调用报出全部不可表达项，让 AI 一轮修完。
pub fn export_ui_scene(spec: &UiSpec, scene_name: &str) -> Result<Scene, Vec<String>> {
    let mut errors = Vec::new();
    let mut names = BTreeSet::new();
    for (node, path) in spec.walk_nodes() {
        if node.key.trim().is_empty() {
            errors.push(format!("{path}.key 不能为空"));
        } else if !names.insert(node.key.as_str()) {
            errors.push(format!(
                "{path}.key 重复：{}（场景实体名必须唯一）",
                node.key
            ));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut scene = Scene::new(scene_name);
    let mut next_id = SceneEntityId(1_u64);
    let root = spec.document.canvas.reference;
    walk(
        spec,
        &spec.root,
        None,
        "root",
        [root[0], root[1]],
        None,
        &mut scene,
        &mut next_id,
        &mut errors,
    );
    if errors.is_empty() {
        validate_components(&scene, &mut errors);
    }
    if errors.is_empty() {
        Ok(scene)
    } else {
        Err(errors)
    }
}

/// 每条导出的组件载荷都过一遍**归属 crate 的解析器**（`scene_codec` 的同一份文法）：
/// 导出件必须能被运行时读回，坏载荷在这里就红，而不是留到宿主上画歪。
fn validate_components(scene: &Scene, errors: &mut Vec<String>) {
    for entity in scene.entities.values() {
        for (name, raw) in &entity.components {
            let result: Result<(), String> = match name.as_str() {
                "UiLayout" => UiRectLayout::from_component_str(raw).map(|_| ()),
                "Container" => ContainerStyle::from_component_str(raw).map(|_| ()),
                "LayoutItem" => LayoutItem::from_component_str(raw).map(|_| ()),
                "Text" => UiText::from_component_str(raw)
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                "Image" => UiImage::from_component_str(raw)
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                "UiEvents" => rustlua_ui::ui_events::UiEvents::parse(raw)
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                other => {
                    errors.push(format!("{}: 导出器发出了未知组件 {other}", entity.name));
                    continue;
                }
            };
            if let Err(error) = result {
                errors.push(format!(
                    "{}: 组件 {name} 载荷不可回读：{error}",
                    entity.name
                ));
            }
        }
    }
}

/// UiSpec → `.scene` 文本（与 `Scene::to_text` 同口径，落盘/产物用）。
pub fn export_scene_text(spec: &UiSpec, scene_name: &str) -> Result<String, Vec<String>> {
    Ok(export_ui_scene(spec, scene_name)?.to_text())
}

/// 从磁盘读一份 `.ui.json` 并导出场景文本。
///
/// revision 是同一份文件字节上的 FNV-1a（`revision_hash`）；产物要对齐 revision 的
/// 话请自行读字节重算——本函数只保证"读到的那份文件导出的是这份场景"。
/// 父节点是否是接管子项摆放的流向容器；`center`/`margin` 按预览器语义没有流向，
/// 子项保持绝对锚定（也不导出 `Container` 组件）。
fn parent_flows(kind: Option<&UiNodeKind>) -> bool {
    matches!(
        kind,
        Some(UiNodeKind::Container {
            direction: ContainerDirection::VBox
                | ContainerDirection::HBox
                | ContainerDirection::Grid
                | ContainerDirection::Wrap,
            ..
        })
    )
}

#[allow(clippy::too_many_arguments)]
fn walk(
    spec: &UiSpec,
    node: &UiNode,
    parent_kind: Option<&UiNodeKind>,
    path: &str,
    parent_rect: [f32; 2],
    parent_id: Option<SceneEntityId>,
    scene: &mut Scene,
    next_id: &mut SceneEntityId,
    errors: &mut Vec<String>,
) {
    let view = StyleView::for_node(spec, node, path, errors);
    // 运行时的 `Container` 接管**全部**子项摆放：没有 `LayoutItem` 的子项会被排到流里、
    // 按默认尺寸取块，绝对内缩无从表达。预览器却允许 offsets 子项退出流向，两者语义在这里
    // 分叉（PC 宿主证据实测到它塌成 0×0），所以 v1 直接拒绝这种形状。
    if parent_flows(parent_kind) && node.layout.offsets.is_some() {
        errors.push(format!(
            "{path}: 流向容器里的子项不能用 offsets 退回绝对定位（运行时会把它排进流并塌成 0×0）"
        ));
    }
    let in_flow = parent_flows(parent_kind) && node.layout.offsets.is_none();
    let mut components: BTreeMap<String, String> = BTreeMap::new();
    if in_flow {
        if let Some(item) = flow_layout_item(&node.layout, parent_rect, path, errors) {
            components.insert("LayoutItem".to_owned(), item.to_component_str());
        }
    } else if let Some(layout) = absolute_layout(&node.layout, parent_rect, path, errors) {
        components.insert("UiLayout".to_owned(), layout.to_component_str());
    }
    if let Some(style) = container_component(&node.kind) {
        components.insert("Container".to_owned(), style.to_component_str());
    }
    emit_visuals(node, &view, path, &mut components, errors);
    emit_events(node, path, &mut components, errors);

    let id = *next_id;
    *next_id = SceneEntityId(id.0 + 1);
    if let Err(error) = scene.add_entity(id, node.key.clone(), parent_id) {
        errors.push(format!("{path}: 建实体失败：{error}"));
        return;
    }
    if let Some(entity) = scene.entities.get_mut(&id) {
        entity.components = components;
    }

    let own_rect = own_rect_for_children(&node.layout, parent_rect, in_flow);
    // 预览器对同一个容器的两类子项用两套参考矩形：流式子项按内框（`padding` 扣掉）
    // 度量与排布，退出流向的（带 `offsets`）仍按完整外框解析。这里必须跟着分叉，
    // 否则 `percent` 尺寸在带内边距的容器里会两边算出不同的烘焙值。
    let flows = parent_flows(Some(&node.kind));
    let inner_rect = inner_size(&node.kind, own_rect);
    for (index, child) in node.children.iter().enumerate() {
        let reference = if flows && child.layout.offsets.is_none() {
            inner_rect
        } else {
            own_rect
        };
        walk(
            spec,
            child,
            Some(&node.kind),
            &format!("{path}.children[{index}]"),
            reference,
            Some(id),
            scene,
            next_id,
            errors,
        );
    }
}

/// 流向容器的内框；没有流向的节点（面板、文本、`center`/`margin` 容器）不扣内边距，
/// 与预览器 `resolve_children` 同口径。
fn inner_size(kind: &UiNodeKind, outer: [f32; 2]) -> [f32; 2] {
    let UiNodeKind::Container { padding, .. } = kind else {
        return outer;
    };
    let pad = padding.unwrap_or([0.0; 4]);
    [
        (outer[0] - pad[0] - pad[2]).max(0.0),
        (outer[1] - pad[1] - pad[3]).max(0.0),
    ]
}

/// 传给子树的自身矩形（只影响 percent 烘焙的参考值）：与预览器同口径，
/// percent 相对外框、流向测量相对内框（`inner_rect`）。
fn own_rect_for_children(layout: &UiLayoutSpec, parent: [f32; 2], in_flow: bool) -> [f32; 2] {
    if let Some(offsets) = layout.offsets {
        [
            (parent[0] - offsets[0] - offsets[2]).max(0.0),
            (parent[1] - offsets[1] - offsets[3]).max(0.0),
        ]
    } else if in_flow {
        measure_child_size(layout, parent)
    } else {
        let size = baked_size(layout, parent);
        [
            if is_stretch(layout.anchor, Axis::X) {
                parent[0]
            } else {
                size[0]
            },
            if is_stretch(layout.anchor, Axis::Y) {
                parent[1]
            } else {
                size[1]
            },
        ]
    }
}

/// 预览器 `resolve_rect` 的 size 口径（size → percent → 默认值，夹 min/max）。
fn baked_size(layout: &UiLayoutSpec, parent: [f32; 2]) -> [f32; 2] {
    let w = layout
        .size
        .map(|size| size[0])
        .or_else(|| {
            layout
                .percent
                .map(|percent| parent[0] * percent[0] / 100.0)
                .filter(|value| *value > 0.0)
        })
        .unwrap_or(SPEC_DEFAULT_W);
    let h = layout
        .size
        .map(|size| size[1])
        .or_else(|| {
            layout
                .percent
                .map(|percent| parent[1] * percent[1] / 100.0)
                .filter(|value| *value > 0.0)
        })
        .unwrap_or(SPEC_DEFAULT_H);
    [w.max(0.0), h.max(0.0)]
}

/// 预览器 `measure_child` 的口径（相对容器内框）。
fn measure_child_size(layout: &UiLayoutSpec, inner: [f32; 2]) -> [f32; 2] {
    let mut size = baked_size(layout, inner);
    if let Some(min) = layout.min_size {
        size[0] = size[0].max(min[0]);
        size[1] = size[1].max(min[1]);
    }
    if let Some(max) = layout.max_size {
        size[0] = size[0].min(max[0]);
        size[1] = size[1].min(max[1]);
    }
    [size[0].max(0.0), size[1].max(0.0)]
}

/// 锚点预设 → 近边 `[x, y]`（与 preview.rs `anchor_fraction` 同一张表）。
fn anchor_fraction(anchor: AnchorPreset) -> [f32; 2] {
    match anchor {
        AnchorPreset::TopLeft => [0.0, 0.0],
        AnchorPreset::TopCenter => [0.5, 0.0],
        AnchorPreset::TopRight => [1.0, 0.0],
        AnchorPreset::CenterLeft => [0.0, 0.5],
        AnchorPreset::Center => [0.5, 0.5],
        AnchorPreset::CenterRight => [1.0, 0.5],
        AnchorPreset::BottomLeft => [0.0, 1.0],
        AnchorPreset::BottomCenter => [0.5, 1.0],
        AnchorPreset::BottomRight => [1.0, 1.0],
        AnchorPreset::StretchHorizontal
        | AnchorPreset::StretchVertical
        | AnchorPreset::FullRect => [0.5, 0.5],
    }
}

fn is_stretch(anchor: AnchorPreset, axis: Axis) -> bool {
    matches!(
        (anchor, axis),
        (
            AnchorPreset::StretchHorizontal | AnchorPreset::FullRect,
            Axis::X
        ) | (
            AnchorPreset::StretchVertical | AnchorPreset::FullRect,
            Axis::Y
        )
    )
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

/// `percent` 的文法边界（与 `LayoutItem::validate` 同口径：0..=100）。
fn percent_valid(layout: &UiLayoutSpec, path: &str, errors: &mut Vec<String>) -> bool {
    let Some(percent) = layout.percent else {
        return true;
    };
    if percent.iter().any(|value| *value < 0.0) {
        errors.push(format!("{path}: percent 不能为负"));
        return false;
    }
    if percent.iter().any(|value| *value > 100.0) {
        errors.push(format!("{path}: percent 不能超过 100"));
        return false;
    }
    true
}

/// 绝对定位节点 → 运行时 `UiRectLayout`；映射不到返回 `None` 并已记 error。
fn absolute_layout(
    layout: &UiLayoutSpec,
    parent: [f32; 2],
    path: &str,
    errors: &mut Vec<String>,
) -> Option<UiRectLayout> {
    if let Some(offsets) = layout.offsets {
        if layout.size.is_some() || layout.percent.is_some() {
            errors.push(format!(
                "{path}: offsets 不能再叠 size/percent（预览按四边内缩，定尺寸无落点）"
            ));
        }
        let (min_size, max_size) = pass_bounds(layout, path, errors);
        // 无边方向夹取会把矩形推离内缩语义；pivot 只在夹取生效时才参与定位，
        // 无 min/max 时透传与预览恒等。
        if layout.pivot.is_some() && (min_size != [0.0, 0.0] || max_size != [0.0, 0.0]) {
            errors.push(format!(
                "{path}: offsets + pivot + min/max 的组合 v1 不可表达"
            ));
        }
        return Some(UiRectLayout {
            anchors: [0.0, 0.0, 1.0, 1.0],
            // Spec 的 `offsets` 是**四边内缩**（预览口径），运行时是"远锚边再加偏移"
            // （`right = parent.w*anchors[2] + offsets[2]`）。右下两条必须取负才等价，
            // 否则 1280 宽的父级会解出 2264 宽——PC 宿主证据实测到的就是这个。
            offsets: [offsets[0], offsets[1], -offsets[2], -offsets[3]],
            min_size,
            max_size,
            pivot: layout.pivot.unwrap_or([0.5, 0.5]),
        });
    }
    let has_live_percent = layout
        .percent
        .is_some_and(|percent| percent[0] > 0.0 || percent[1] > 0.0)
        && layout.size.is_none();
    if !percent_valid(layout, path, errors) {
        return None;
    }
    let anchor = anchor_fraction(layout.anchor);
    let pivot = layout.pivot.unwrap_or(anchor);
    if pivot.iter().any(|value| *value < 0.0 || *value > 1.0) {
        errors.push(format!("{path}: pivot 必须在 0..=1"));
        return None;
    }
    let position = layout.position.unwrap_or([0.0, 0.0]);
    let (min_size, max_size) = pass_bounds(layout, path, errors);
    let mut anchors = [0.0_f32; 4];
    let mut offsets = [0.0_f32; 4];
    for axis in [Axis::X, Axis::Y] {
        let (i, near_edge, far_edge, name) = match axis {
            Axis::X => (0_usize, 0_usize, 2_usize, "x"),
            Axis::Y => (1, 1, 3, "y"),
        };
        if is_stretch(layout.anchor, axis) {
            if position[i] != 0.0 || min_size[i] != 0.0 || max_size[i] != 0.0 {
                errors.push(format!(
                    "{path}: stretch 轴不支持 position/min_size/max_size（内缩表达不了居中位移与无边夹取）"
                ));
                return None;
            }
            anchors[near_edge] = 0.0;
            anchors[far_edge] = 1.0;
            continue;
        }
        let percent = if has_live_percent {
            layout.percent.unwrap_or([0.0, 0.0])[i]
        } else {
            0.0
        };
        if percent > 0.0 {
            // 线性锚边表达 `anchor*f + p*(1-2*pivot)`：宽 = (far-near)*parent。
            let fraction = percent / 100.0;
            let near = anchor[i] - fraction * pivot[i];
            let far = near + fraction;
            if !(0.0..=1.0).contains(&near) || !(0.0..=1.0).contains(&far) {
                errors.push(format!(
                    "{path}: percent/pivot 组合把{name}轴锚边推出 0..=1（near={near}, far={far}），v1 不可表达"
                ));
                return None;
            }
            anchors[near_edge] = near;
            anchors[far_edge] = far;
            // percent 轴的锚点项由 near/far 自己承载（w = (far-near)*parent），
            // offsets 只剩 position——再把 anchor*parent 烧进去就双计了。
            offsets[near_edge] = position[i];
            offsets[far_edge] = position[i];
        } else {
            let size = baked_size(layout, parent)[i];
            let edge = anchor[i] * parent[i] + position[i] - size * pivot[i];
            anchors[near_edge] = 0.0;
            anchors[far_edge] = 0.0;
            offsets[near_edge] = edge;
            offsets[far_edge] = edge + size;
        }
    }
    Some(UiRectLayout {
        anchors,
        offsets,
        min_size,
        max_size,
        pivot,
    })
}

fn pass_bounds(
    layout: &UiLayoutSpec,
    path: &str,
    errors: &mut Vec<String>,
) -> ([f32; 2], [f32; 2]) {
    let mut min = [0.0_f32; 2];
    let mut max = [0.0_f32; 2];
    if let Some(values) = layout.min_size {
        if values.iter().any(|value| *value < 0.0) {
            errors.push(format!("{path}: min_size 不能为负"));
        } else {
            min = values;
        }
    }
    if let Some(values) = layout.max_size {
        if values.iter().any(|value| *value < 0.0) {
            errors.push(format!("{path}: max_size 不能为负"));
        } else {
            max = values;
        }
    }
    (min, max)
}

/// 流向子项 → `LayoutItem`：percent 轴保持活口径（相对内框，与预览同形），
/// 其余轴把预览测量烘焙成 `preferred_size`；grow/shrink/min/max/align_self 直通。
fn flow_layout_item(
    layout: &UiLayoutSpec,
    parent: [f32; 2],
    path: &str,
    errors: &mut Vec<String>,
) -> Option<LayoutItem> {
    if !percent_valid(layout, path, errors) {
        return None;
    }
    let mut item = LayoutItem {
        grow: layout.grow.unwrap_or(0.0).max(0.0),
        shrink: layout.shrink.unwrap_or(1.0).max(0.0),
        size_percent: layout.percent.unwrap_or([0.0, 0.0]),
        ..LayoutItem::default()
    };
    if let Some(min) = layout.min_size {
        item.min_size = min;
    }
    if let Some(max) = layout.max_size {
        item.max_size = max;
    }
    if let Some(align) = layout.align_self {
        item.align_self = Some(match align {
            Align::Start => ChildAlign::Start,
            Align::Center => ChildAlign::Center,
            Align::End => ChildAlign::End,
            Align::Stretch => {
                errors.push(format!(
                    "{path}.align_self: stretch 在 v1 导出不可表达（fill 是容器全局口径）"
                ));
                return None;
            }
        });
    }
    // percent 轴保持活口径（preferred 置 0 = 让位给 percent）；其余轴烘焙预览测量值。
    let measured = measure_child_size(layout, parent);
    let percent = layout.percent.unwrap_or([0.0, 0.0]);
    for axis in 0..2 {
        if percent[axis] <= 0.0 {
            item.preferred_size[axis] = measured[axis];
        }
    }
    Some(item)
}

/// `Container` 组件：方向/内边距/间距/列数/对齐/分布。`center`/`margin` 不导出
/// 容器组件（预览语义 = 子项绝对锚定，运行时 Center/Margin 容器却会接管摆放）。
fn container_component(kind: &UiNodeKind) -> Option<ContainerStyle> {
    let UiNodeKind::Container {
        direction,
        padding,
        spacing,
        justify,
        align,
        columns,
    } = kind
    else {
        return None;
    };
    let style_kind = match direction {
        ContainerDirection::VBox => ContainerKind::VBox,
        ContainerDirection::HBox => ContainerKind::HBox,
        ContainerDirection::Grid => ContainerKind::Grid,
        ContainerDirection::Wrap => ContainerKind::Wrap,
        ContainerDirection::Margin | ContainerDirection::Center => return None,
    };
    Some(ContainerStyle {
        kind: style_kind,
        padding: padding.unwrap_or([0.0; 4]),
        spacing: spacing.unwrap_or(0.0),
        columns: columns.unwrap_or(0) as usize,
        stretch: matches!(align, Align::Stretch),
        fill: matches!(align, Align::Stretch),
        align: match align {
            Align::Start | Align::Stretch => ChildAlign::Start,
            Align::Center => ChildAlign::Center,
            Align::End => ChildAlign::End,
        },
        justify: match justify {
            SpecJustify::Start => Justify::Start,
            SpecJustify::Center => Justify::Center,
            SpecJustify::End => Justify::End,
            SpecJustify::SpaceBetween => Justify::SpaceBetween,
            SpecJustify::SpaceAround => Justify::SpaceAround,
            SpecJustify::SpaceEvenly => Justify::SpaceEvenly,
        },
    })
}

/// 样式里的颜色键。缺失 → `None`；写了但解析不成颜色 → 报错并进 `None`。
/// 不退回白色：白退路会把坏颜色伪装成一次成功的导出。
fn style_color(
    view: &StyleView,
    key: &str,
    path: &str,
    errors: &mut Vec<String>,
) -> Option<[f32; 4]> {
    let value = view.get(key)?;
    match parse_rgba(value, &view.tokens) {
        Some(color) => Some(color),
        None => {
            errors.push(format!(
                "{path}.style.{key}: 解析不成颜色（{value}），v1 不退回白色也不静默丢弃"
            ));
            None
        }
    }
}

fn emit_visuals(
    node: &UiNode,
    view: &StyleView,
    path: &str,
    components: &mut BTreeMap<String, String>,
    errors: &mut Vec<String>,
) {
    let has_bg = view.get("bg").is_some() || view.get("background").is_some();
    let tint = style_color(view, "bg", path, errors)
        .or_else(|| style_color(view, "background", path, errors))
        .unwrap_or([1.0, 1.0, 1.0, 1.0]);
    let text_color = style_color(view, "color", path, errors).unwrap_or([1.0, 1.0, 1.0, 1.0]);
    let opacity = view
        .get("opacity")
        .and_then(Value::as_f64)
        .map_or(1.0, |value| value as f32);
    let font_size = view
        .get("font_size")
        .and_then(Value::as_f64)
        .map_or(UI_TEXT_BASE_PX, |value| value as f32);
    if font_size <= 0.0 {
        errors.push(format!("{path}.style: font_size 必须大于 0"));
    }
    let font = view
        .get("font")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let nine_slice = nine_slice_from_style(view);
    if view.get("nine_slice").is_some() && nine_slice.is_none() {
        errors.push(format!(
            "{path}.style.nine_slice: 必须是 4 个数字，否则切片会丢"
        ));
    }
    let tinted_block = UiImage {
        texture: "__white__".to_owned(),
        color: [tint[0], tint[1], tint[2], tint[3] * opacity],
        nine_slice,
    }
    .to_component_str();
    match &node.kind {
        UiNodeKind::Panel { src } => {
            // 面板的可见性只有两种真源：`src` 指到的贴图，或样式里显式写了底色。
            // 都没写就不建 Image——预览那层占位灰是画刷，不是文档内容，导出它等于造假数据。
            if let Some(texture) = src {
                components.insert(
                    "Image".to_owned(),
                    UiImage {
                        texture: texture.clone(),
                        color: [tint[0], tint[1], tint[2], tint[3] * opacity],
                        nine_slice,
                    }
                    .to_component_str(),
                );
            } else if has_bg {
                components.insert("Image".to_owned(), tinted_block);
            }
        }
        UiNodeKind::Container { .. } => {
            if has_bg {
                components.insert("Image".to_owned(), tinted_block);
            }
        }
        UiNodeKind::Text { text } => {
            if text.is_empty() {
                errors.push(format!("{path}: text 节点缺少文字载荷"));
            }
            // 文本也能有底色（标题条、标签胶囊）：写了 `bg` 就同一实体上叠一块白图，
            // 与按钮同一形状。丢掉它等于文档里写了底色、宿主上什么都没有。
            if has_bg {
                components.insert("Image".to_owned(), tinted_block);
            }
            components.insert(
                "Text".to_owned(),
                UiText {
                    text: text.clone(),
                    font_size,
                    color: [text_color[0], text_color[1], text_color[2]],
                    font,
                }
                .to_component_str(),
            );
        }
        UiNodeKind::Image { src, nine_slice } => {
            components.insert(
                "Image".to_owned(),
                UiImage {
                    texture: src.clone(),
                    color: [tint[0], tint[1], tint[2], tint[3] * opacity],
                    nine_slice: *nine_slice,
                }
                .to_component_str(),
            );
        }
        UiNodeKind::Button { text, src } => {
            // 按钮 = 同一实体上的 Image（无 src 用内建白块）+ Text + UiEvents。
            if text.as_deref().unwrap_or("").is_empty() {
                errors.push(format!(
                    "{path}: button 节点 v1 要求 text 非空（无载荷的按钮不可点验）"
                ));
            }
            components.insert(
                "Image".to_owned(),
                UiImage {
                    texture: src.clone().unwrap_or_else(|| "__white__".to_owned()),
                    color: [tint[0], tint[1], tint[2], tint[3] * opacity],
                    nine_slice,
                }
                .to_component_str(),
            );
            if let Some(text) = text.as_ref().filter(|text| !text.is_empty()) {
                components.insert(
                    "Text".to_owned(),
                    UiText {
                        text: text.clone(),
                        font_size,
                        color: [text_color[0], text_color[1], text_color[2]],
                        font,
                    }
                    .to_component_str(),
                );
            }
        }
        other => {
            errors.push(format!(
                "{path}: v1 导出只支持 panel/container/text/image/button，不支持 {}",
                other.type_name()
            ));
        }
    }
}

fn nine_slice_from_style(view: &StyleView) -> Option<[f32; 4]> {
    view.get("nine_slice")
        .and_then(|value| value.as_array())
        .filter(|items| items.len() == 4)
        .map(|items| parse_f32x4(items.as_slice()))
}

fn parse_f32x4(items: &[Value]) -> [f32; 4] {
    let mut out = [0.0_f32; 4];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = item.as_f64().map_or(0.0, |value| value as f32);
    }
    out
}

/// 事件：键必须是运行时认识的事件名（`UiEventType::parse`），函数名原样带过去。
fn emit_events(
    node: &UiNode,
    path: &str,
    components: &mut BTreeMap<String, String>,
    errors: &mut Vec<String>,
) {
    if node.events.is_empty() {
        return;
    }
    let mut bindings = Vec::new();
    for (event, function) in &node.events {
        if UiEventType::parse(event).is_none() {
            errors.push(format!("{path}.events: 运行时不认识事件 {event:?}"));
            continue;
        }
        if function.trim().is_empty() {
            errors.push(format!("{path}.events.{event} 的函数名为空"));
            continue;
        }
        bindings.push(format!("event={event};function={function}"));
    }
    if !bindings.is_empty() {
        components.insert("UiEvents".to_owned(), bindings.join("|"));
    }
}

/// 颜色：`"#RRGGBB[AA]"` 或 `[r,g,b[,a]]`（0..=1），先做 `@token` 解析。
fn parse_rgba(value: &Value, tokens: &BTreeMap<String, Value>) -> Option<[f32; 4]> {
    let value = match value {
        Value::String(text) if text.starts_with('@') => tokens.get(&text[1..])?,
        other => other,
    };
    if let Some(text) = value.as_str() {
        let hex = text.trim().strip_prefix('#')?;
        let channel = |range: std::ops::Range<usize>| -> Option<f32> {
            u8::from_str_radix(&hex[range], 16)
                .map(|byte| byte as f32 / 255.0)
                .ok()
        };
        return match hex.len() {
            6 => Some([channel(0..2)?, channel(2..4)?, channel(4..6)?, 1.0]),
            8 => Some([
                channel(0..2)?,
                channel(2..4)?,
                channel(4..6)?,
                channel(6..8)?,
            ]),
            _ => None,
        };
    }
    let items = value.as_array()?;
    // 数组形态按 8 位通道读（与预览器 `Color([r, g, b, a])` 同一口径），组件侧才是 0..1。
    let byte = |item: &Value| -> Option<f32> {
        let value = item.as_f64()?;
        (0.0..=255.0)
            .contains(&value)
            .then_some(value as f32 / 255.0)
    };
    match items.len() {
        3 => Some([byte(&items[0])?, byte(&items[1])?, byte(&items[2])?, 1.0]),
        4 => Some([
            byte(&items[0])?,
            byte(&items[1])?,
            byte(&items[2])?,
            byte(&items[3])?,
        ]),
        _ => None,
    }
}

/// 节点生效样式：`styles[node.style].base`（值可 `@token` 引用）。
/// v1 只消费 bg/background/color/font/font_size/opacity/nine_slice，其余键忽略；
/// 带状态样式（hover/pressed/…）的节点报错——运行时按钮态是 `ButtonStates`，
/// 映射规则不是"基样式盖一层"能概括的。
/// v1 能落成组件载荷的样式键。其余键（`radius` / `border_*` / `text_align` / `padding` …）
/// 在 `.scene` 侧没有对应字段，出现即报错：静默丢样式等于导出一份骗人的证据。
const SUPPORTED_STYLE_KEYS: [&str; 7] = [
    "background",
    "bg",
    "color",
    "font",
    "font_size",
    "nine_slice",
    "opacity",
];

struct StyleView {
    values: BTreeMap<String, Value>,
    tokens: BTreeMap<String, Value>,
}

impl StyleView {
    fn for_node(spec: &UiSpec, node: &UiNode, path: &str, errors: &mut Vec<String>) -> Self {
        let mut values = BTreeMap::new();
        if let Some(style) = &node.style {
            let Some(definition) = spec.styles.get(style) else {
                errors.push(format!("{path}.style 引用了不存在的样式：{style}"));
                return Self {
                    values,
                    tokens: spec.tokens.clone(),
                };
            };
            if !definition.states.is_empty() {
                errors.push(format!(
                    "{path}.style {style}: v1 导出不支持状态样式（{} 个状态会丢）",
                    definition.states.len()
                ));
            }
            values = definition.base.clone();
            for key in values.keys() {
                if !SUPPORTED_STYLE_KEYS.contains(&key.as_str()) {
                    errors.push(format!(
                        "{path}.style {style}: v1 没有样式键 {key:?} 的落点（写了会丢），请改用贴图/`Image` 表达"
                    ));
                }
            }
        }
        Self {
            values,
            tokens: spec.tokens.clone(),
        }
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_ui_spec::{DocumentKind, UiDocumentMeta, UiStyle};

    fn node(raw: Value) -> UiNode {
        serde_json::from_value(raw).unwrap()
    }

    fn spec_of(root: Value) -> UiSpec {
        let root = node(root);
        UiSpec {
            spec_version: 1,
            document: UiDocumentMeta {
                kind: DocumentKind::Widget,
                name: "Matrix".to_owned(),
                canvas: Default::default(),
            },
            tokens: BTreeMap::new(),
            styles: BTreeMap::new(),
            root,
            assets: Vec::new(),
            bindings: Vec::new(),
        }
    }

    fn layout_of(fields: Value) -> UiLayoutSpec {
        // `fields` 本身就是 layout 对象——再包一层会被 serde 当未知字段丢掉。
        node(serde_json::json!({
            "key": "n", "type": "text", "text": "x", "layout": fields
        }))
        .layout
    }

    fn layout_errors(layout: Value) -> Vec<String> {
        let mut errors = Vec::new();
        let _ = absolute_layout(&layout_of(layout), [1280.0, 720.0], "x.layout", &mut errors);
        errors
    }

    /// offsets 形态：spec 写四边**内缩**，运行时远锚边是"锚 + 偏移"，所以右下取负才等价。
    /// 直通不取负时 1280×720 的父级会解出 2264×1312——这是 PC 宿主证据实测出来的红。
    #[test]
    fn offsets_export_negates_the_far_edges() {
        let layout = layout_of(serde_json::json!({"offsets": [10.0, 20.0, 30.0, 40.0]}));
        let mut errors = Vec::new();
        let exported = absolute_layout(&layout, [1280.0, 720.0], "x", &mut errors).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(exported.anchors, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(exported.offsets, [10.0, 20.0, -30.0, -40.0]);
        let rect = exported.resolve([1280.0, 720.0]);
        assert_eq!(
            (rect.x, rect.y, rect.w, rect.h),
            (10.0, 20.0, 1240.0, 660.0)
        );
        assert!(UiRectLayout::from_component_str(&exported.to_component_str()).is_ok());
    }

    /// offsets 再叠 size/percent 是歧义形态，v1 拒绝。
    #[test]
    fn offsets_with_size_or_percent_errors() {
        assert!(
            !layout_errors(
                serde_json::json!({"offsets": [1.0, 1.0, 1.0, 1.0], "size": [10.0, 10.0]})
            )
            .is_empty()
        );
        assert!(
            !layout_errors(
                serde_json::json!({"offsets": [1.0, 1.0, 1.0, 1.0], "percent": [50.0, 0.0]})
            )
            .is_empty()
        );
    }

    /// 点锚 + 定尺寸烘焙成双锚矩形，解析回读后与预览同值。
    #[test]
    fn point_anchor_bakes_edges() {
        let layout = layout_of(serde_json::json!({"anchor": "top_center", "size": [200.0, 40.0]}));
        let mut errors = Vec::new();
        let exported = absolute_layout(&layout, [1280.0, 720.0], "x", &mut errors).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert!(UiRectLayout::from_component_str(&exported.to_component_str()).is_ok());
        let rect = exported.resolve([1280.0, 720.0]);
        assert_eq!((rect.x, rect.y, rect.w, rect.h), (540.0, 0.0, 200.0, 40.0));
    }

    /// percent 用"锚边随 pivot 滑动"表达：`anchor*f + p*(1-2*pivot)`，相对外框。
    #[test]
    fn percent_axes_use_sliding_anchors() {
        let layout =
            layout_of(serde_json::json!({"anchor": "top_center", "percent": [50.0, 10.0]}));
        let mut errors = Vec::new();
        let exported = absolute_layout(&layout, [1280.0, 720.0], "x", &mut errors).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert!(UiRectLayout::from_component_str(&exported.to_component_str()).is_ok());
        let rect = exported.resolve([1280.0, 720.0]);
        assert_eq!((rect.x, rect.y, rect.w, rect.h), (320.0, 0.0, 640.0, 72.0));
    }

    /// stretch 轴内缩到整父；带 position/min/max 的都是 v1 不可表达，必须报错。
    #[test]
    fn stretch_is_representable_only_without_insets() {
        let plain = layout_of(serde_json::json!({"anchor": "full_rect", "size": [10.0, 10.0]}));
        let mut errors = Vec::new();
        let exported = absolute_layout(&plain, [1280.0, 720.0], "x", &mut errors).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let rect = exported.resolve([1280.0, 720.0]);
        assert_eq!((rect.x, rect.y, rect.w, rect.h), (0.0, 0.0, 1280.0, 720.0));

        assert!(
            !layout_errors(serde_json::json!({"anchor": "full_rect", "position": [4.0, 0.0]}))
                .is_empty()
        );
        assert!(
            !layout_errors(serde_json::json!({
                "anchor": "stretch_horizontal",
                "size": [10.0, 10.0],
                "min_size": [10.0, 0.0]
            }))
            .is_empty()
        );
    }

    /// percent/pivot 把锚边推出 0..=1 时报错，绝不退化。
    #[test]
    fn percent_out_of_range_anchor_errors() {
        // top_right + pivot 贴左边：远锚边 = 1 + 60% 越界。
        assert!(
            !layout_errors(serde_json::json!({
                "anchor": "top_right", "percent": [60.0, 0.0], "pivot": [0.0, 0.0]
            }))
            .is_empty()
        );
        assert!(!layout_errors(serde_json::json!({"percent": [150.0, 0.0]})).is_empty());
    }

    /// 端到端：矩阵文档（标题 + vbox 容器 + 流向子项 + 按钮）导出可回读、可解算，
    /// 且与预览器的矩形一致。
    #[test]
    fn end_to_end_matrix_doc_exports_and_resolves() {
        let spec = spec_of(serde_json::json!({
            "key": "root", "type": "panel",
            "layout": {"anchor": "full_rect"},
            "children": [
                {"key": "title", "type": "text", "text": "HP",
                 "layout": {"anchor": "top_center", "size": [200.0, 40.0]}},
                {"key": "box", "type": "container",
                 "direction": "v_box", "padding": [8.0, 8.0, 8.0, 8.0], "spacing": 4.0,
                 "layout": {"anchor": "center", "size": [300.0, 120.0]},
                 "children": [
                     {"key": "row", "type": "text", "text": "row",
                      "layout": {"size": [100.0, 20.0]}}
                 ]},
                {"key": "ok", "type": "button", "text": "OK",
                 "events": {"click": "on_ok"},
                 "layout": {"anchor": "bottom_right", "size": [120.0, 32.0]}}
            ]
        }));
        let scene = export_ui_scene(&spec, "ui_matrix").unwrap();
        assert_eq!(scene.entities.len(), 5);
        assert_eq!(
            scene.entities.keys().copied().collect::<Vec<_>>(),
            (1..=5_u64).map(SceneEntityId).collect::<Vec<_>>()
        );
        assert_eq!(scene.entities[&SceneEntityId(1)].name, "root");
        assert_eq!(
            scene.entities[&SceneEntityId(5)].parent,
            Some(SceneEntityId(1))
        );
        for entity in scene.entities.values() {
            for (name, raw) in &entity.components {
                let parsed = match name.as_str() {
                    "UiLayout" => UiRectLayout::from_component_str(raw).map(|_| ()),
                    "Container" => ContainerStyle::from_component_str(raw).map(|_| ()),
                    "LayoutItem" => LayoutItem::from_component_str(raw).map(|_| ()),
                    "UiEvents" => rustlua_ui::ui_events::UiEvents::parse(raw)
                        .map(|_| ())
                        .map_err(|error| error.to_string()),
                    "Text" => UiText::from_component_str(raw)
                        .map(|_| ())
                        .map_err(|error| error.to_string()),
                    "Image" => UiImage::from_component_str(raw)
                        .map(|_| ())
                        .map_err(|error| error.to_string()),
                    other => panic!("未知组件 {other}"),
                };
                assert!(parsed.is_ok(), "{name}={raw}: {parsed:?}");
            }
        }
        let button = &scene.entities[&SceneEntityId(5)];
        assert_eq!(
            button.components.keys().cloned().collect::<Vec<_>>(),
            vec!["Image", "Text", "UiEvents", "UiLayout"]
        );
        assert!(button.components["Image"].starts_with("texture=__white__"));
        assert_eq!(button.components["UiEvents"], "event=click;function=on_ok");

        // 预览 vs 运行时解算逐键对照（文本固有尺寸不参与：全部键都有显式几何）。
        assert_rects_match_preview(&spec, &scene);
    }

    /// 把导出的场景喂给运行时解算器，逐键对齐预览器的矩形。
    fn assert_rects_match_preview(spec: &UiSpec, scene: &Scene) {
        let nodes = rustlua_ui::ui_scene_layout::build_ui_scene_nodes(scene.entities.iter().map(
            |(id, entity)| rustlua_ui::ui_scene_layout::UiSceneNodeSource {
                key: id.0,
                parent: entity.parent.map(|parent| parent.0),
                components: &entity.components,
            },
        ));
        let (resolved, diagnostics) =
            rustlua_ui::ui_scene_layout::resolve_scene_ui_layout(&nodes, [1280.0, 720.0]);
        assert!(
            diagnostics.iter().all(|item| matches!(
                item.reason,
                rustlua_ui::ui_scene_layout::UiLayoutReason::Overflow { .. }
            )),
            "{diagnostics:?}"
        );
        for item in rustlua_ui_spec::preview::layout_rects(spec) {
            let id = scene
                .entities
                .values()
                .find(|entity| entity.name == item.key)
                .unwrap()
                .id;
            let rect = resolved[&id.0].rect;
            for (actual, expected) in [
                (rect.x, item.rect[0]),
                (rect.y, item.rect[1]),
                (rect.w, item.rect[2]),
                (rect.h, item.rect[3]),
            ] {
                assert!(
                    (actual - expected).abs() < 0.51,
                    "{}: runtime {actual} vs preview {expected}",
                    item.key
                );
            }
        }
    }

    /// 带内边距的流向容器里，`percent` 子项相对**内框**取尺寸，它的绝对锚定子项也按
    /// 内框算边——参考矩形分叉错了这里就红。
    #[test]
    fn padded_flow_container_uses_inner_rect_for_children() {
        let spec = spec_of(serde_json::json!({
            "key": "root", "type": "panel", "layout": {"anchor": "full_rect"},
            "children": [{
                "key": "box", "type": "container", "direction": "v_box",
                "padding": [10.0, 10.0, 10.0, 10.0], "spacing": 0.0,
                "layout": {"anchor": "top_left", "size": [300.0, 120.0]},
                "children": [{
                    "key": "half", "type": "panel", "layout": {"percent": [50.0, 50.0]},
                    "children": [{
                        "key": "corner", "type": "text", "text": "c",
                        "layout": {"anchor": "bottom_right", "size": [60.0, 20.0]}
                    }]
                }]
            }]
        }));
        let scene = export_ui_scene(&spec, "inner").unwrap();
        // 内框 280×100 → half = 140×50；corner 贴 half 右下 = 140-60 / 50-20。
        let layout = |name: &str| {
            let entity = scene
                .entities
                .values()
                .find(|entity| entity.name == name)
                .unwrap();
            UiRectLayout::from_component_str(&entity.components["UiLayout"]).unwrap()
        };
        assert_eq!(layout("corner").offsets[0], 80.0);
        assert_eq!(layout("corner").offsets[1], 30.0);
        assert_rects_match_preview(&spec, &scene);
    }

    /// 流向容器里的子项写 `offsets`：预览让它退出流向，运行时却照样排进流并塌成 0×0，
    /// 两边语义分叉 ⇒ v1 必须拒绝（宿主实测红过一次）。
    #[test]
    fn offsets_child_inside_flow_container_is_rejected() {
        let spec = spec_of(serde_json::json!({
            "key": "root", "type": "panel", "layout": {"anchor": "full_rect"},
            "children": [{
                "key": "box", "type": "container", "direction": "v_box",
                "layout": {"anchor": "top_left", "size": [200.0, 100.0]},
                "children": [{
                    "key": "escaped", "type": "panel",
                    "layout": {"offsets": [10.0, 10.0, 10.0, 10.0]}
                }]
            }]
        }));
        let errors = export_ui_scene(&spec, "s").unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("流向容器里的子项")),
            "{errors:?}"
        );
        // 同一形状挂在 `center` 容器（没有流向）下是合法的绝对定位。
        let centered = serde_json::json!({
            "key": "root", "type": "panel", "layout": {"anchor": "full_rect"},
            "children": [{
                "key": "box", "type": "container", "direction": "center",
                "layout": {"anchor": "top_left", "size": [200.0, 100.0]},
                "children": [{
                    "key": "escaped", "type": "panel",
                    "layout": {"offsets": [10.0, 10.0, 10.0, 10.0]}
                }]
            }]
        });
        assert!(export_ui_scene(&spec_of(centered), "s").is_ok());
    }

    /// v2 节点（toggle）必须报错而不是退化导出。
    #[test]
    fn unsupported_kind_is_an_error() {
        let spec = spec_of(serde_json::json!({"key": "t", "type": "toggle", "checked": true}));
        let errors = export_ui_scene(&spec, "s").unwrap_err();
        assert!(errors.iter().any(|e| e.contains("toggle")), "{errors:?}");
    }

    /// key 重复直接拒绝（`ui_rects_by_name` 按名取值，重名即二义）。
    #[test]
    fn duplicate_keys_are_rejected() {
        let spec = spec_of(serde_json::json!({
            "key": "root", "type": "panel",
            "children": [
                {"key": "x", "type": "text", "text": "a"},
                {"key": "x", "type": "text", "text": "b"}
            ]
        }));
        let errors = export_ui_scene(&spec, "s").unwrap_err();
        assert!(errors.iter().any(|e| e.contains("重复")), "{errors:?}");
    }

    /// 状态样式必须报错（按钮态是 ButtonStates 的映射，v1 不做盖层近似）。
    #[test]
    fn state_styles_are_rejected() {
        let mut spec = spec_of(serde_json::json!({
            "key": "b", "type": "button", "text": "OK", "style": "card"
        }));
        spec.styles.insert(
            "card".to_owned(),
            UiStyle {
                base: BTreeMap::new(),
                states: [(rustlua_ui_spec::StyleState::Hover, BTreeMap::new())]
                    .into_iter()
                    .collect(),
            },
        );
        let errors = export_ui_scene(&spec, "s").unwrap_err();
        assert!(errors.iter().any(|e| e.contains("状态样式")), "{errors:?}");
    }

    /// `.scene` 侧没有落点的样式键（`radius`/`border_*`/`text_align`…）必须报错：
    /// 静默丢掉等于交出一份"看着对、其实少画了东西"的宿主证据。
    #[test]
    fn unmappable_style_keys_are_rejected() {
        let mut spec = spec_of(serde_json::json!({
            "key": "b", "type": "button", "text": "OK", "style": "card"
        }));
        spec.styles.insert(
            "card".to_owned(),
            UiStyle {
                base: [("radius".to_owned(), serde_json::json!(14.0))]
                    .into_iter()
                    .collect(),
                states: BTreeMap::new(),
            },
        );
        let errors = export_ui_scene(&spec, "s").unwrap_err();
        assert!(errors.iter().any(|e| e.contains("radius")), "{errors:?}");
    }

    /// 底色是面板/容器唯一的可见来源之一：写了才建白块，没写就不建（预览那层占位灰
    /// 是画刷，不是文档内容）。
    #[test]
    fn declared_background_emits_a_white_block_only_when_written() {
        let mut spec = spec_of(serde_json::json!({
            "key": "root", "type": "panel",
            "layout": {"anchor": "full_rect"},
            "children": [
                {"key": "tinted", "type": "container", "direction": "v_box", "style": "card",
                 "layout": {"anchor": "top_left", "size": [100.0, 40.0]}},
                {"key": "tinted_label", "type": "text", "text": "hi", "style": "card",
                 "layout": {"anchor": "top_left", "size": [100.0, 40.0], "position": [0.0, 50.0]}},
                {"key": "bare", "type": "panel",
                 "layout": {"anchor": "top_left", "size": [100.0, 40.0]}}
            ]
        }));
        spec.styles.insert(
            "card".to_owned(),
            UiStyle {
                base: [(
                    "bg".to_owned(),
                    serde_json::json!([20.0, 30.0, 40.0, 255.0]),
                )]
                .into_iter()
                .collect(),
                states: BTreeMap::new(),
            },
        );
        let scene = export_ui_scene(&spec, "s").unwrap();
        let image = |name: &str| {
            scene
                .entities
                .values()
                .find(|entity| entity.name == name)
                .unwrap()
                .components
                .get("Image")
                .cloned()
        };
        assert_eq!(
            image("tinted").as_deref(),
            Some("texture=__white__;color=0.078,0.118,0.157")
        );
        assert_eq!(image("bare"), None);
        // 文本写底色 = 同一实体上 Image + Text（与按钮同一形状），底色不能只落在文字上。
        let label = scene
            .entities
            .values()
            .find(|entity| entity.name == "tinted_label")
            .unwrap();
        assert_eq!(
            label.components.keys().cloned().collect::<Vec<_>>(),
            vec!["Image", "Text", "UiLayout"]
        );
    }

    /// 令牌在导出时解析成字面量进组件串（bg → Image.color）。
    #[test]
    fn style_tokens_resolve_into_component_literals() {
        let mut spec = spec_of(serde_json::json!({
            "key": "p", "type": "image", "src": "assets/ui/a.png", "style": "card",
            "layout": {"offsets": [0.0, 0.0, 100.0, 50.0]}
        }));
        spec.tokens.insert(
            "color.surface".to_owned(),
            Value::String("#1E1E24".to_owned()),
        );
        spec.styles.insert(
            "card".to_owned(),
            UiStyle {
                base: [("bg".to_owned(), Value::String("@color.surface".to_owned()))]
                    .into_iter()
                    .collect(),
                states: BTreeMap::new(),
            },
        );
        let scene = export_ui_scene(&spec, "s").unwrap();
        assert_eq!(
            scene.entities[&SceneEntityId(1)].components["Image"],
            "texture=assets/ui/a.png;color=0.118,0.118,0.141"
        );
    }

    /// 未知事件名必须报错——运行时收到就丢。
    #[test]
    fn unknown_event_name_errors() {
        let spec = spec_of(serde_json::json!({
            "key": "b", "type": "button", "text": "OK",
            "events": {"bogus": "f"}
        }));
        let errors = export_ui_scene(&spec, "s").unwrap_err();
        assert!(errors.iter().any(|e| e.contains("bogus")), "{errors:?}");
    }

    /// 导出文本可被场景解析器原样读回（落盘契约）。
    #[test]
    fn scene_text_round_trips() {
        let spec = spec_of(serde_json::json!({
            "key": "root", "type": "panel",
            "layout": {"anchor": "full_rect"},
            "children": [{"key": "t", "type": "text", "text": "hi",
                          "layout": {"percent": [50.0, 0.0]}}]
        }));
        let text = export_scene_text(&spec, "round").unwrap();
        let parsed = Scene::from_text(&text).unwrap();
        assert_eq!(parsed, export_ui_scene(&spec, "round").unwrap());
    }
}
