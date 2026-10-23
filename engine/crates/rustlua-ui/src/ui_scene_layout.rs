//! E16/E17: shared hierarchy resolution for scene UI layouts.

use std::collections::{BTreeMap, BTreeSet};

use super::ui_layout::{
    ChildLayout, ContainerKind, ContainerStyle, LayoutAxis, LayoutItem, UiRect, UiRectLayout,
    layout_container_with_report,
};

/// 宿主每帧判断"运行期 spawn 的布局节点有没有变"要靠它比较，因此 `PartialEq`
/// 是这条链要求的能力（`UiRectLayout`/`ContainerStyle` 都已可比较）。
#[derive(Clone, Debug, PartialEq)]
pub struct UiSceneNode {
    pub parent: Option<u64>,
    pub layout: Option<UiRectLayout>,
    pub container: Option<ContainerStyle>,
    /// Per-child constraints authored on a `LayoutItem` component (P3-E05).
    pub layout_item: Option<LayoutItem>,
    /// Measured visual size used by containers when the node has no stretch
    /// layout. Non-layout entities keep their measured size at the parent origin.
    pub preferred_size: [f32; 2],
}

/// 决定一个实体是否参与场景 UI 布局的组件名。
///
/// 这张表只有一份：场景快照与运行期 prefab spawn 都读它，否则"场景里的 UI 会画、
/// 同一份数据 spawn 出来不画"就成了第二处真源。
pub const UI_LAYOUT_COMPONENTS: [&str; 7] = [
    "UiLayout",
    "Container",
    "LayoutItem",
    "Text",
    "Image",
    "UiEvents",
    "ButtonStates",
];

/// 一份实体文档里参与 UI 布局所需的最小信息。
///
/// 刻意不接 `Scene`/`Prefab` 类型：调用方自己决定 key 是什么（场景快照用压缩后的
/// 实体序，等于活实体槽位；运行期 spawn 直接用落进世界的槽位），从而 scene 文档与
/// prefab 文档共用同一套建节点规则。
pub struct UiSceneNodeSource<'a> {
    pub key: u64,
    pub parent: Option<u64>,
    pub components: &'a BTreeMap<String, String>,
}

/// Host-supplied intrinsic size for one UI entity (P3-E05-3).
///
/// Precedence (single definition for runtime, prefab spawn and editor):
/// `Image` keeps its authored `UiLayout` rect because the renderer stretches
/// the texture into that rect — PNG pixels are not a layout input; `Text` uses
/// the host's measured glyph box; everything else keeps the 64×64 placeholder.
/// The UI crate cannot depend on the renderer, so text metrics come in through
/// `measure_text`.
pub fn preferred_size_for(
    components: &BTreeMap<String, String>,
    layout: Option<&UiRectLayout>,
    measure_text: &mut dyn FnMut(&str, f32) -> Option<[f32; 2]>,
) -> [f32; 2] {
    if components.contains_key("Image") {
        return layout
            .map(|layout| {
                let rect = layout.resolve([0.0, 0.0]);
                [rect.w, rect.h]
            })
            .unwrap_or([64.0, 64.0]);
    }
    components
        .get("Text")
        .and_then(|raw| super::ui::UiText::from_component_str(raw).ok())
        .and_then(|text| measure_text(&text.text, text.effective_scale()))
        .unwrap_or([64.0, 64.0])
}

/// 从组件字符串建 UI 布局节点表。**只为参与场景 UI 的实体建节点**：带 UI 组件的实体，
/// 以及它们的祖先与后代。祖先用于解析父链，后代是因为容器的子节点自身可能没有 UI 组件
/// 却被容器摆放。其余实体不建节点——否则每帧的布局解算会随**总实体数**增长
/// （`wp0-v4` 实测 10k 纯精灵场景每帧多出 3,339 次分配，正是这张全量节点表造成的）。
pub fn build_ui_scene_nodes<'a>(
    sources: impl IntoIterator<Item = UiSceneNodeSource<'a>>,
) -> BTreeMap<u64, UiSceneNode> {
    build_ui_scene_nodes_with_intrinsic(sources, &mut |_, _| None)
}

/// [`build_ui_scene_nodes`] with host text metrics: the renderer's built-in
/// bitmap measurement turns `Text` components into real intrinsic sizes
/// (E05-3) instead of the 64×64 placeholder.
pub fn build_ui_scene_nodes_with_intrinsic<'a>(
    sources: impl IntoIterator<Item = UiSceneNodeSource<'a>>,
    measure_text: &mut dyn FnMut(&str, f32) -> Option<[f32; 2]>,
) -> BTreeMap<u64, UiSceneNode> {
    let sources: Vec<UiSceneNodeSource<'_>> = sources.into_iter().collect();
    let mut relevant = BTreeSet::new();
    for source in &sources {
        if UI_LAYOUT_COMPONENTS
            .iter()
            .any(|name| source.components.contains_key(*name))
        {
            relevant.insert(source.key);
        }
    }
    let mut children: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for source in &sources {
        if let Some(parent) = source.parent {
            children.entry(parent).or_default().push(source.key);
        }
    }
    // 祖先 + 后代闭包：从 UI 实体出发双向扩张。
    let mut frontier = relevant.iter().copied().collect::<Vec<_>>();
    let parent_of = sources
        .iter()
        .map(|source| (source.key, source.parent))
        .collect::<BTreeMap<_, _>>();
    while let Some(key) = frontier.pop() {
        if let Some(&Some(parent)) = parent_of.get(&key)
            && relevant.insert(parent)
        {
            frontier.push(parent);
        }
        if let Some(kids) = children.get(&key) {
            for kid in kids {
                if relevant.insert(*kid) {
                    frontier.push(*kid);
                }
            }
        }
    }
    let mut nodes = BTreeMap::new();
    for source in &sources {
        if !relevant.contains(&source.key) {
            continue;
        }
        let layout = source
            .components
            .get("UiLayout")
            .and_then(|raw| UiRectLayout::from_component_str(raw).ok());
        let container = source
            .components
            .get("Container")
            .and_then(|raw| ContainerStyle::from_component_str(raw).ok());
        let layout_item = source
            .components
            .get("LayoutItem")
            .and_then(|raw| LayoutItem::from_component_str(raw).ok());
        let preferred_size = preferred_size_for(source.components, layout.as_ref(), measure_text);
        nodes.insert(
            source.key,
            UiSceneNode {
                parent: source.parent,
                layout,
                container,
                layout_item,
                preferred_size,
            },
        );
    }
    nodes
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedUiNode {
    pub rect: UiRect,
    /// True when a parent container placed this node; manual offsets are then
    /// expected to be treated read-only by editors.
    pub container_controlled: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiLayoutDiagnostic {
    pub entity: u64,
    pub reason: UiLayoutReason,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UiLayoutReason {
    MissingParent,
    CircularParent,
    InvalidLayout,
    InvalidContainer,
    /// Container content exceeds its padded area after shrink.
    Overflow {
        axis: LayoutAxis,
        needed: f32,
        available: f32,
    },
}

/// Resolves nodes in parent-first order. A root's parent is `canvas_size`
/// (viewport or UiRoot reference resolution). Nodes without `UiLayout` use
/// their preferred size at the parent origin, which keeps non-layout entities
/// visible while containers can still place them.
///
/// 两遍语义：容器子项的尺寸先自底向上测量（子容器 = 内容 + 内边距），父容器再自顶向下
/// 摆放，并把摆放结果递归传给子容器。测量结果在一次解算内按子树缓存，深度嵌套不会重复测量。
pub fn resolve_scene_ui_layout(
    nodes: &BTreeMap<u64, UiSceneNode>,
    canvas_size: [f32; 2],
) -> (BTreeMap<u64, ResolvedUiNode>, Vec<UiLayoutDiagnostic>) {
    let (resolved, diagnostics, _) = resolve_scene_ui_layout_with_stats(nodes, canvas_size);
    (resolved, diagnostics)
}

/// Bottom-up measurement counters for one resolve pass (P3-E05-3).
///
/// `hits` is the number of subtree measurements served from the per-pass cache;
/// `misses` is the number actually computed. Reachable only by containers: a
/// child container is measured while arranging its parent and then again when
/// the resolver reaches it, which is exactly the reuse this cache exists for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeasureStats {
    pub hits: usize,
    pub misses: usize,
}

/// Same as [`resolve_scene_ui_layout`] but also returns measurement counters.
pub fn resolve_scene_ui_layout_with_stats(
    nodes: &BTreeMap<u64, UiSceneNode>,
    canvas_size: [f32; 2],
) -> (
    BTreeMap<u64, ResolvedUiNode>,
    Vec<UiLayoutDiagnostic>,
    MeasureStats,
) {
    let mut resolved: BTreeMap<u64, ResolvedUiNode> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut measured = MeasureCache {
        nodes,
        sizes: BTreeMap::new(),
        visiting: BTreeSet::new(),
        stats: MeasureStats::default(),
    };
    let mut arranged = BTreeSet::new();

    for id in nodes.keys().copied() {
        resolve_node(
            nodes,
            canvas_size,
            id,
            &mut resolved,
            &mut diagnostics,
            &mut visiting,
            &mut visited,
            &mut measured,
            &mut arranged,
        );
    }
    let stats = measured.stats;
    (resolved, diagnostics, stats)
}

/// 一次解算内的自底向上测量缓存。容器尺寸取决于子树内容，叶子的 `preferred_size`
/// 是宿主给的固有尺寸（文本/图片/占位）。
struct MeasureCache<'a> {
    nodes: &'a BTreeMap<u64, UiSceneNode>,
    sizes: BTreeMap<u64, [f32; 2]>,
    visiting: BTreeSet<u64>,
    stats: MeasureStats,
}

impl MeasureCache<'_> {
    fn measure(&mut self, id: u64) -> [f32; 2] {
        if let Some(size) = self.sizes.get(&id) {
            self.stats.hits += 1;
            return *size;
        }
        if !self.visiting.insert(id) {
            // 父链成环时按零尺寸兜底；环本身由摆放阶段的诊断报告。环不是一次测量，
            // 既不算命中也不算未命中。
            return [0.0, 0.0];
        }
        let (base, item) = match self.nodes.get(&id) {
            None => ([0.0, 0.0], None),
            Some(node) => {
                let base = match &node.container {
                    None => node.preferred_size,
                    Some(style) => self.measure_container(id, style),
                };
                (base, node.layout_item)
            }
        };
        // LayoutItem constraints shape the intrinsic size too; percent sizes
        // stay unresolved here because the parent width is not known yet.
        let size = item.map_or(base, |item| {
            item.child_layout(base).resolved_size([0.0, 0.0])
        });
        self.visiting.remove(&id);
        self.sizes.insert(id, size);
        self.stats.misses += 1;
        size
    }

    fn measure_container(&mut self, id: u64, style: &ContainerStyle) -> [f32; 2] {
        let child_ids: Vec<u64> = self
            .nodes
            .iter()
            .filter(|(_, child)| child.parent == Some(id))
            .map(|(child_id, _)| *child_id)
            .collect();
        let mut sizes: Vec<[f32; 2]> = Vec::new();
        for child_id in child_ids {
            let visible = self
                .nodes
                .get(&child_id)
                .and_then(|node| node.layout_item)
                .is_none_or(|item| item.visible);
            if visible {
                sizes.push(self.measure(child_id));
            }
        }
        let count = sizes.len();
        let gaps = style.spacing * count.saturating_sub(1) as f32;
        let content = if count == 0 {
            [0.0, 0.0]
        } else {
            match style.kind {
                ContainerKind::VBox => [
                    sizes.iter().map(|size| size[0]).fold(0.0_f32, f32::max),
                    sizes.iter().map(|size| size[1]).sum::<f32>() + gaps,
                ],
                // Wrap measures like an unconstrained row; the container width
                // decides the actual row breaks during arrangement.
                ContainerKind::HBox | ContainerKind::Wrap => [
                    sizes.iter().map(|size| size[0]).sum::<f32>() + gaps,
                    sizes.iter().map(|size| size[1]).fold(0.0_f32, f32::max),
                ],
                ContainerKind::Grid => {
                    let columns = if style.columns > 0 {
                        style.columns
                    } else {
                        (count as f32).sqrt().ceil().max(1.0) as usize
                    };
                    let rows = count.div_ceil(columns);
                    let mut column_widths = vec![0.0_f32; columns];
                    let mut row_heights = vec![0.0_f32; rows];
                    for (index, size) in sizes.iter().enumerate() {
                        let column = index % columns;
                        let row = index / columns;
                        column_widths[column] = column_widths[column].max(size[0]);
                        row_heights[row] = row_heights[row].max(size[1]);
                    }
                    [
                        column_widths.iter().sum::<f32>()
                            + style.spacing * columns.saturating_sub(1) as f32,
                        row_heights.iter().sum::<f32>()
                            + style.spacing * rows.saturating_sub(1) as f32,
                    ]
                }
                ContainerKind::Margin | ContainerKind::Center => [
                    sizes.iter().map(|size| size[0]).fold(0.0_f32, f32::max),
                    sizes.iter().map(|size| size[1]).fold(0.0_f32, f32::max),
                ],
            }
        };
        [
            content[0] + style.padding[0] + style.padding[2],
            content[1] + style.padding[1] + style.padding[3],
        ]
    }
}

/// 自顶向下摆放容器的直接子项，并对子容器继续递归。容器摆放结果优先于子项自身的
/// `UiLayout`——否则嵌套容器的外层摆放会被内层覆盖。`arranged` 防止父链成环时无限递归。
#[allow(clippy::too_many_arguments)]
fn arrange_children(
    nodes: &BTreeMap<u64, UiSceneNode>,
    id: u64,
    rect: UiRect,
    resolved: &mut BTreeMap<u64, ResolvedUiNode>,
    diagnostics: &mut Vec<UiLayoutDiagnostic>,
    measured: &mut MeasureCache<'_>,
    visited: &mut BTreeSet<u64>,
    arranged: &mut BTreeSet<u64>,
) {
    let Some(style) = nodes.get(&id).and_then(|node| node.container.as_ref()) else {
        return;
    };
    if !arranged.insert(id) {
        return;
    }
    let mut children: Vec<(u64, [f32; 2])> = Vec::new();
    for (child_id, child) in nodes {
        if child.parent != Some(id) {
            continue;
        }
        let size = if child.container.is_some() {
            measured.measure(*child_id)
        } else {
            child.preferred_size
        };
        children.push((*child_id, size));
    }
    let layouts: Vec<ChildLayout> = children
        .iter()
        .map(|(child_id, size)| {
            nodes
                .get(child_id)
                .and_then(|node| node.layout_item)
                .map_or_else(
                    || ChildLayout::sized(*size),
                    |item| item.child_layout(*size),
                )
        })
        .collect();
    let report = layout_container_with_report(rect, &layouts, style);
    for overflow in report.overflows {
        diagnostics.push(UiLayoutDiagnostic {
            entity: id,
            reason: UiLayoutReason::Overflow {
                axis: overflow.axis,
                needed: overflow.needed,
                available: overflow.available,
            },
        });
    }
    for ((child_id, _), (layout, child_rect)) in children
        .into_iter()
        .zip(layouts.into_iter().zip(report.rects))
    {
        let child_rect = UiRect::new(
            rect.x + child_rect.x,
            rect.y + child_rect.y,
            child_rect.w,
            child_rect.h,
        );
        resolved.insert(
            child_id,
            ResolvedUiNode {
                rect: child_rect,
                container_controlled: true,
            },
        );
        visited.insert(child_id);
        if !layout.visible {
            hide_descendants(nodes, child_id, resolved, visited);
            continue;
        }
        arrange_children(
            nodes,
            child_id,
            child_rect,
            resolved,
            diagnostics,
            measured,
            visited,
            arranged,
        );
    }
}

/// A hidden `LayoutItem` hides the whole subtree: descendants keep zero rects
/// instead of being arranged at the parent origin.
fn hide_descendants(
    nodes: &BTreeMap<u64, UiSceneNode>,
    id: u64,
    resolved: &mut BTreeMap<u64, ResolvedUiNode>,
    visited: &mut BTreeSet<u64>,
) {
    for (child_id, child) in nodes {
        if child.parent != Some(id) || !visited.insert(*child_id) {
            continue;
        }
        resolved.insert(
            *child_id,
            ResolvedUiNode {
                rect: UiRect::ZERO,
                container_controlled: true,
            },
        );
        hide_descendants(nodes, *child_id, resolved, visited);
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_node(
    nodes: &BTreeMap<u64, UiSceneNode>,
    canvas_size: [f32; 2],
    id: u64,
    resolved: &mut BTreeMap<u64, ResolvedUiNode>,
    diagnostics: &mut Vec<UiLayoutDiagnostic>,
    visiting: &mut BTreeSet<u64>,
    visited: &mut BTreeSet<u64>,
    measured: &mut MeasureCache<'_>,
    arranged: &mut BTreeSet<u64>,
) {
    if visited.contains(&id) {
        return;
    }
    if !visiting.insert(id) {
        diagnostics.push(UiLayoutDiagnostic {
            entity: id,
            reason: UiLayoutReason::CircularParent,
        });
        return;
    }
    let Some(node) = nodes.get(&id) else {
        diagnostics.push(UiLayoutDiagnostic {
            entity: id,
            reason: UiLayoutReason::MissingParent,
        });
        visiting.remove(&id);
        return;
    };
    let (parent_rect, parent_diagnostic) = match node.parent {
        None => (UiRect::new(0.0, 0.0, canvas_size[0], canvas_size[1]), None),
        Some(parent_id) if nodes.contains_key(&parent_id) => {
            resolve_node(
                nodes,
                canvas_size,
                parent_id,
                resolved,
                diagnostics,
                visiting,
                visited,
                measured,
                arranged,
            );
            let rect = resolved
                .get(&parent_id)
                .map_or_else(|| UiRect::ZERO, |parent| parent.rect);
            (rect, None)
        }
        Some(_) => (
            UiRect::new(0.0, 0.0, canvas_size[0], canvas_size[1]),
            Some(UiLayoutDiagnostic {
                entity: id,
                reason: UiLayoutReason::MissingParent,
            }),
        ),
    };
    if let Some(diagnostic) = parent_diagnostic {
        diagnostics.push(diagnostic);
    }
    // 父容器在递归里已经把本节点摆放好：容器结果优先，不再用本节点的 UiLayout 覆盖。
    if visited.contains(&id) {
        visiting.remove(&id);
        return;
    }

    let mut rect = node
        .layout
        .as_ref()
        .map(|layout| layout.resolve([parent_rect.w, parent_rect.h]))
        .unwrap_or_else(|| {
            // 无 UiLayout 的容器填满父矩形（根容器=画布）；普通元素保持固有尺寸，
            // 偏移只加一次（父矩形可能非零）。
            if node.container.is_some() {
                UiRect::new(0.0, 0.0, parent_rect.w, parent_rect.h)
            } else {
                UiRect::new(0.0, 0.0, node.preferred_size[0], node.preferred_size[1])
            }
        });
    rect.x += parent_rect.x;
    rect.y += parent_rect.y;

    resolved.insert(
        id,
        ResolvedUiNode {
            rect,
            container_controlled: false,
        },
    );
    visited.insert(id);
    arrange_children(
        nodes,
        id,
        rect,
        resolved,
        diagnostics,
        measured,
        visited,
        arranged,
    );
    visiting.remove(&id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_layout::CanvasConfig;
    use crate::ui_layout::ContainerKind;
    use crate::ui_layout::UiLayoutPreset;

    fn node(parent: Option<u64>, size: [f32; 2]) -> UiSceneNode {
        UiSceneNode {
            parent,
            layout: None,
            container: None,
            layout_item: None,
            preferred_size: size,
        }
    }

    #[test]
    fn resolves_root_and_point_layout_rects() {
        let mut nodes = BTreeMap::new();
        let mut root = node(None, [100.0, 100.0]);
        root.layout = Some(UiRectLayout::from_preset(
            UiLayoutPreset::Center,
            [-50.0, -25.0, 50.0, 25.0],
        ));
        nodes.insert(1, root);
        let (resolved, diagnostics) = resolve_scene_ui_layout(&nodes, [1280.0, 720.0]);
        assert!(diagnostics.is_empty());
        assert_eq!(resolved[&1].rect, UiRect::new(590.0, 335.0, 100.0, 50.0));
    }

    #[test]
    fn container_places_children_relative_to_resolved_parent() {
        let mut nodes = BTreeMap::new();
        let mut root = node(None, [400.0, 200.0]);
        root.layout = Some(UiRectLayout {
            anchors: [0.0, 0.0, 1.0, 0.0],
            offsets: [10.0, 10.0, -10.0, 110.0],
            ..UiRectLayout::default()
        });
        root.container = Some(ContainerStyle {
            kind: ContainerKind::HBox,
            spacing: 10.0,
            ..ContainerStyle::default()
        });
        nodes.insert(1, root);
        nodes.insert(2, node(Some(1), [100.0, 50.0]));
        nodes.insert(3, node(Some(1), [80.0, 50.0]));
        let (resolved, diagnostics) = resolve_scene_ui_layout(&nodes, [1280.0, 720.0]);
        assert!(diagnostics.is_empty());
        assert_eq!(resolved[&1].rect, UiRect::new(10.0, 10.0, 1260.0, 100.0));
        assert_eq!(resolved[&2].rect, UiRect::new(10.0, 10.0, 100.0, 50.0));
        assert_eq!(resolved[&3].rect, UiRect::new(120.0, 10.0, 80.0, 50.0));
        assert!(resolved[&2].container_controlled);
        assert!(resolved[&3].container_controlled);
    }

    #[test]
    fn reports_missing_and_circular_parents() {
        let mut nodes = BTreeMap::new();
        nodes.insert(1, node(Some(2), [10.0, 10.0]));
        nodes.insert(2, node(Some(1), [10.0, 10.0]));
        nodes.insert(3, node(Some(99), [10.0, 10.0]));
        let (_, diagnostics) = resolve_scene_ui_layout(&nodes, [1280.0, 720.0]);
        assert!(diagnostics.contains(&UiLayoutDiagnostic {
            entity: 1,
            reason: UiLayoutReason::CircularParent,
        }));
        assert!(diagnostics.contains(&UiLayoutDiagnostic {
            entity: 3,
            reason: UiLayoutReason::MissingParent,
        }));
    }

    #[test]
    fn canvas_config_supplies_root_size_for_resolution_preview() {
        let canvas = CanvasConfig::default();
        let viewport = [1920.0, 1080.0];
        let scale = canvas.scale(viewport);
        assert_eq!(scale, [1.5, 1.5]);
        assert_eq!(canvas.safe_rect(viewport).w, 1920.0);
    }

    fn container_node(parent: Option<u64>, style: ContainerStyle) -> UiSceneNode {
        UiSceneNode {
            parent,
            layout: None,
            container: Some(style),
            layout_item: None,
            preferred_size: [64.0, 64.0],
        }
    }

    /// E05-1：三层嵌套容器按「先测内容、再摆放」得到正确矩形；id 乱序（子先于父）
    /// 也必须由容器摆放决定，不能被子节点自身的 `UiLayout` 覆盖。
    #[test]
    fn nested_containers_measure_then_arrange_three_levels() {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            50,
            container_node(
                None,
                ContainerStyle {
                    kind: ContainerKind::VBox,
                    padding: [10.0; 4],
                    spacing: 5.0,
                    ..ContainerStyle::default()
                },
            ),
        );
        nodes.insert(90, node(Some(50), [50.0, 20.0]));
        let mut middle = container_node(
            Some(50),
            ContainerStyle {
                kind: ContainerKind::HBox,
                padding: [5.0; 4],
                spacing: 2.0,
                ..ContainerStyle::default()
            },
        );
        // 内层容器自身的 UiLayout 只作固有尺寸的退路；被父容器摆放时不得覆盖摆放结果。
        middle.layout = Some(UiRectLayout {
            anchors: [0.0, 0.0, 0.0, 0.0],
            offsets: [0.0, 0.0, 10.0, 10.0],
            ..UiRectLayout::default()
        });
        nodes.insert(10, middle);
        nodes.insert(7, node(Some(10), [100.0, 40.0]));
        nodes.insert(3, node(Some(10), [80.0, 60.0]));

        let (resolved, diagnostics) = resolve_scene_ui_layout(&nodes, [1280.0, 720.0]);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        // 内层 intrinsic：HBox 宽 100+80+2，高 max(40,60)，再加 padding 5×2 = 192×70。
        assert_eq!(resolved[&10].rect, UiRect::new(10.0, 10.0, 192.0, 70.0));
        // 根 VBox：首个孩子（内层容器）在 padding 内，第二个孩子在其下 5px。
        assert_eq!(resolved[&90].rect, UiRect::new(10.0, 85.0, 50.0, 20.0));
        // 内层容器再摆它的孩子：padding 5，按 id 顺序 3 → 7。
        assert_eq!(resolved[&3].rect, UiRect::new(15.0, 15.0, 80.0, 60.0));
        assert_eq!(resolved[&7].rect, UiRect::new(97.0, 15.0, 100.0, 40.0));
        assert!(resolved[&10].container_controlled);
        assert!(resolved[&3].container_controlled);
        assert!(resolved[&7].container_controlled);
    }

    /// 无 `UiLayout` 的子节点落在父矩形原点，父偏移只能加一次。
    #[test]
    fn layout_less_child_sits_at_parent_origin_once() {
        let mut nodes = BTreeMap::new();
        let mut parent = node(None, [10.0, 10.0]);
        parent.layout = Some(UiRectLayout {
            anchors: [0.0, 0.0, 0.0, 0.0],
            offsets: [60.0, 120.0, 160.0, 170.0],
            ..UiRectLayout::default()
        });
        nodes.insert(1, parent);
        nodes.insert(2, node(Some(1), [40.0, 20.0]));
        let (resolved, diagnostics) = resolve_scene_ui_layout(&nodes, [1280.0, 720.0]);
        assert!(diagnostics.is_empty());
        assert_eq!(resolved[&1].rect, UiRect::new(60.0, 120.0, 100.0, 50.0));
        assert_eq!(resolved[&2].rect, UiRect::new(60.0, 120.0, 40.0, 20.0));
        assert!(!resolved[&2].container_controlled);
    }

    /// 容器父链成环时两遍布局必须终止并报诊断（不能因为递归摆放而挂死）。
    #[test]
    fn circular_container_parents_terminate() {
        let mut nodes = BTreeMap::new();
        nodes.insert(1, container_node(Some(2), ContainerStyle::default()));
        nodes.insert(2, container_node(Some(1), ContainerStyle::default()));
        let (resolved, diagnostics) = resolve_scene_ui_layout(&nodes, [1280.0, 720.0]);
        assert!(resolved.contains_key(&1) && resolved.contains_key(&2));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.entity == 1 && diagnostic.reason == UiLayoutReason::CircularParent
        }));
    }

    /// P3-E05：`LayoutItem.grow` 在 `stretch=false` 时也生效，且按父容器内边距摆放。
    #[test]
    fn layout_item_grows_child_inside_container() {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            1,
            container_node(
                None,
                ContainerStyle {
                    kind: ContainerKind::HBox,
                    padding: [10.0; 4],
                    ..ContainerStyle::default()
                },
            ),
        );
        let mut growing = node(Some(1), [100.0, 40.0]);
        growing.layout_item = Some(LayoutItem {
            preferred_size: [100.0, 0.0],
            grow: 1.0,
            ..LayoutItem::default()
        });
        nodes.insert(2, growing);
        nodes.insert(3, node(Some(1), [100.0, 40.0]));

        let (resolved, diagnostics) = resolve_scene_ui_layout(&nodes, [400.0, 300.0]);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(resolved[&2].rect, UiRect::new(10.0, 10.0, 280.0, 40.0));
        assert_eq!(resolved[&3].rect, UiRect::new(290.0, 10.0, 100.0, 40.0));
    }

    /// P3-E05：装不下的内容必须产出 Overflow 诊断，而不是静默裁剪。
    #[test]
    fn container_overflow_becomes_a_diagnostic() {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            1,
            container_node(
                None,
                ContainerStyle {
                    kind: ContainerKind::HBox,
                    ..ContainerStyle::default()
                },
            ),
        );
        let mut rigid = node(Some(1), [500.0, 20.0]);
        rigid.layout_item = Some(LayoutItem {
            shrink: 0.0,
            ..LayoutItem::default()
        });
        nodes.insert(2, rigid);

        let (_, diagnostics) = resolve_scene_ui_layout(&nodes, [400.0, 300.0]);
        assert!(
            diagnostics.contains(&UiLayoutDiagnostic {
                entity: 1,
                reason: UiLayoutReason::Overflow {
                    axis: LayoutAxis::Horizontal,
                    needed: 500.0,
                    available: 400.0,
                },
            }),
            "{diagnostics:?}"
        );
    }

    /// P3-E05：同一条容器数据在三种分辨率下产出确定的绝对矩形。
    /// 仓库没有截图基线，这份 golden 钉的是布局解算读数（百分比宽度随画布变化）。
    #[test]
    fn three_resolution_layout_rect_golden() {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            1,
            container_node(
                None,
                ContainerStyle {
                    kind: ContainerKind::VBox,
                    padding: [8.0; 4],
                    spacing: 10.0,
                    ..ContainerStyle::default()
                },
            ),
        );
        nodes.insert(2, node(Some(1), [100.0, 40.0]));
        nodes.insert(
            3,
            container_node(
                Some(1),
                ContainerStyle {
                    kind: ContainerKind::HBox,
                    padding: [4.0; 4],
                    spacing: 4.0,
                    ..ContainerStyle::default()
                },
            ),
        );
        nodes.insert(4, node(Some(3), [100.0, 20.0]));
        nodes.insert(5, node(Some(3), [50.0, 30.0]));
        let mut percent = node(Some(1), [64.0, 20.0]);
        percent.layout_item = Some(LayoutItem {
            size_percent: [50.0, 0.0],
            ..LayoutItem::default()
        });
        nodes.insert(6, percent);

        let cases = [
            // 画布、百分比子项的绝对矩形。
            ([1280.0, 720.0], UiRect::new(8.0, 106.0, 632.0, 20.0)),
            ([1920.0, 1080.0], UiRect::new(8.0, 106.0, 952.0, 20.0)),
            ([1080.0, 1920.0], UiRect::new(8.0, 106.0, 532.0, 20.0)),
        ];
        for (canvas, percent_rect) in cases {
            let (resolved, diagnostics) = resolve_scene_ui_layout(&nodes, canvas);
            assert!(diagnostics.is_empty(), "canvas {canvas:?}");
            // 根 VBox：padding 8，孩子按 id 顺序 2 → 3 → 6。
            assert_eq!(resolved[&2].rect, UiRect::new(8.0, 8.0, 100.0, 40.0));
            // 内层 HBox intrinsic：100+4+50+padding 8 = 162 宽，30+8 = 38 高。
            assert_eq!(resolved[&3].rect, UiRect::new(8.0, 58.0, 162.0, 38.0));
            assert_eq!(resolved[&4].rect, UiRect::new(12.0, 62.0, 100.0, 20.0));
            assert_eq!(resolved[&5].rect, UiRect::new(116.0, 62.0, 50.0, 30.0));
            assert_eq!(resolved[&6].rect, percent_rect);
        }
    }

    /// 一次解算里子容器被父容器测量一次、随后摆放自己时再取一次——第二次必须走缓存。
    #[test]
    fn measure_stats_count_container_cache_reuse() {
        let mut nodes = BTreeMap::new();
        nodes.insert(1, container_node(None, ContainerStyle::default()));
        nodes.insert(2, container_node(Some(1), ContainerStyle::default()));
        nodes.insert(3, container_node(Some(2), ContainerStyle::default()));
        nodes.insert(4, node(Some(3), [10.0, 10.0]));
        let (resolved, diagnostics, stats) =
            resolve_scene_ui_layout_with_stats(&nodes, [400.0, 300.0]);
        assert!(diagnostics.is_empty());
        assert_eq!(resolved.len(), 4);
        // 容器 2、3 与叶子 4 各测量一次；容器 3 在摆放容器 2 时二次取值命中缓存。
        assert_eq!(stats, MeasureStats { hits: 1, misses: 3 });
    }

    /// 文本走宿主度量；图片取作者矩形且不触发文本度量；两者都缺才用占位尺寸。
    #[test]
    fn preferred_size_prefers_text_intrinsic_then_image_layout() {
        let mut text_components = BTreeMap::new();
        text_components.insert("Text".to_owned(), "text=ABCD;font_size=16".to_owned());
        let mut measured = Vec::new();
        let text_size = preferred_size_for(&text_components, None, &mut |text, _scale| {
            measured.push(text.to_owned());
            Some([40.0, 12.0])
        });
        assert_eq!(text_size, [40.0, 12.0]);
        assert_eq!(measured, ["ABCD"]);

        let mut image_components = BTreeMap::new();
        image_components.insert("Image".to_owned(), "texture=__white__".to_owned());
        let layout = UiRectLayout {
            anchors: [0.0; 4],
            offsets: [10.0, 20.0, 70.0, 60.0],
            ..UiRectLayout::default()
        };
        let image_size = preferred_size_for(&image_components, Some(&layout), &mut |_, _| {
            panic!("Image must not invoke text measurement")
        });
        assert_eq!(image_size, [60.0, 40.0]);

        let empty = BTreeMap::new();
        assert_eq!(
            preferred_size_for(&empty, None, &mut |_, _| None),
            [64.0, 64.0]
        );
    }
}
