//! E16/E17: shared hierarchy resolution for scene UI layouts.

use std::collections::{BTreeMap, BTreeSet};

use super::ui_layout::{ChildLayout, ContainerStyle, UiRect, UiRectLayout, layout_container};

#[derive(Clone, Debug)]
pub struct UiSceneNode {
    pub parent: Option<u64>,
    pub layout: Option<UiRectLayout>,
    pub container: Option<ContainerStyle>,
    /// Measured visual size used by containers when the node has no stretch
    /// layout. Non-layout entities keep their measured size at the parent origin.
    pub preferred_size: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedUiNode {
    pub rect: UiRect,
    /// True when a parent container placed this node; manual offsets are then
    /// expected to be treated read-only by editors.
    pub container_controlled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiLayoutDiagnostic {
    pub entity: u64,
    pub reason: UiLayoutReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiLayoutReason {
    MissingParent,
    CircularParent,
    InvalidLayout,
    InvalidContainer,
}

/// Resolves nodes in parent-first order. A root's parent is `canvas_size`
/// (viewport or UiRoot reference resolution). Nodes without `UiLayout` use
/// their preferred size at the parent origin, which keeps non-layout entities
/// visible while containers can still place them.
pub fn resolve_scene_ui_layout(
    nodes: &BTreeMap<u64, UiSceneNode>,
    canvas_size: [f32; 2],
) -> (BTreeMap<u64, ResolvedUiNode>, Vec<UiLayoutDiagnostic>) {
    let mut resolved: BTreeMap<u64, ResolvedUiNode> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();

    for id in nodes.keys().copied() {
        resolve_node(
            nodes,
            canvas_size,
            id,
            &mut resolved,
            &mut diagnostics,
            &mut visiting,
            &mut visited,
        );
    }
    (resolved, diagnostics)
}

fn resolve_node(
    nodes: &BTreeMap<u64, UiSceneNode>,
    canvas_size: [f32; 2],
    id: u64,
    resolved: &mut BTreeMap<u64, ResolvedUiNode>,
    diagnostics: &mut Vec<UiLayoutDiagnostic>,
    visiting: &mut BTreeSet<u64>,
    visited: &mut BTreeSet<u64>,
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

    let mut rect = node
        .layout
        .as_ref()
        .map(|layout| layout.resolve([parent_rect.w, parent_rect.h]))
        .unwrap_or_else(|| {
            UiRect::new(
                parent_rect.x,
                parent_rect.y,
                node.preferred_size[0],
                node.preferred_size[1],
            )
        });
    rect.x += parent_rect.x;
    rect.y += parent_rect.y;
    let container_controlled = false;

    if let Some(style) = &node.container {
        let children = nodes
            .iter()
            .filter(|(_, child)| child.parent == Some(id))
            .map(|(child_id, child)| (*child_id, ChildLayout::sized(child.preferred_size)))
            .collect::<Vec<_>>();
        let child_layouts = children.iter().map(|(_, child)| *child).collect::<Vec<_>>();
        let rects = layout_container(rect, &child_layouts, style);
        for ((child_id, _), child_rect) in children.iter().zip(rects) {
            let child_rect = UiRect::new(
                rect.x + child_rect.x,
                rect.y + child_rect.y,
                child_rect.w,
                child_rect.h,
            );
            resolved.insert(
                *child_id,
                ResolvedUiNode {
                    rect: child_rect,
                    container_controlled: true,
                },
            );
            visited.insert(*child_id);
        }
    }

    resolved.insert(
        id,
        ResolvedUiNode {
            rect,
            container_controlled,
        },
    );
    visited.insert(id);
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
}
