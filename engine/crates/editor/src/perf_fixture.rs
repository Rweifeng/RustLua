//! Repeatable editor performance fixtures (docs/55 阶段 0 / ER-01).
//!
//! Fixtures are plain scenes/strings so the same specification can drive unit tests, the
//! headless `--perf-baseline` report, and manual GUI baselining. Nothing here depends on
//! GPU state, so measurements are portable across machines and CI.

use crate::SceneDocument;
use engine::core::{math3d::Vec3, scene::SceneEntityId, transform::Transform3D};

/// Named fixture sizes used by the baseline report and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixtureSize {
    /// 100 entities: quick smoke runs and unit tests.
    Small,
    /// 1_000 entities: everyday project scale.
    Medium,
    /// 10_000 entities: the plan's Hierarchy acceptance target.
    Large,
}

impl FixtureSize {
    pub fn spec(self) -> FixtureSpec {
        match self {
            Self::Small => FixtureSpec {
                entities: 100,
                children_per_node: 4,
                console_lines: 1_000,
            },
            Self::Medium => FixtureSpec {
                entities: 1_000,
                children_per_node: 5,
                console_lines: 10_000,
            },
            Self::Large => FixtureSpec {
                entities: 10_000,
                children_per_node: 5,
                console_lines: 100_000,
            },
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

/// Deterministic fixture description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixtureSpec {
    /// Total number of entities to create (including the roots).
    pub entities: usize,
    /// Maximum children per node; breadth keeps the tree realistic and recursion shallow.
    pub children_per_node: usize,
    /// Number of synthetic console/log lines to generate.
    pub console_lines: usize,
}

impl FixtureSpec {
    pub fn small() -> Self {
        FixtureSize::Small.spec()
    }
    pub fn medium() -> Self {
        FixtureSize::Medium.spec()
    }
    pub fn large() -> Self {
        FixtureSize::Large.spec()
    }
}

/// A built fixture: an unsaved scene document plus expectations used by assertions.
#[derive(Clone, Debug)]
pub struct SceneFixture {
    pub document: SceneDocument,
    /// Number of entities the flatten step must produce with nothing collapsed.
    pub expected_rows: usize,
}

impl SceneFixture {
    /// Entity ids in creation order (roots first, then breadth-first children).
    pub fn entity_ids(&self) -> Vec<SceneEntityId> {
        let mut ids: Vec<SceneEntityId> = self.document.scene.entities.keys().copied().collect();
        ids.sort_unstable_by_key(|id| id.0);
        ids
    }
}

/// Build a scene with `spec.entities` entities in a deterministic breadth-first hierarchy.
///
/// Every entity gets a `Transform3D` component so fixture scenes exercise the same typed
/// component path as real 3D projects (`ROOT-i` nodes are transform-less roots).
pub fn build_scene_fixture(spec: FixtureSpec) -> SceneFixture {
    let mut document = SceneDocument::new_scene("perf_fixture");
    let entities = spec.entities.max(1);
    let fan_out = spec.children_per_node.max(1);
    // Root count: keep breadth-first allocation simple and deterministic.
    let root_count = fan_out.min(entities);
    let mut ids: Vec<SceneEntityId> = Vec::with_capacity(entities);
    let mut next_id = 1_u64;

    for index in 0..root_count {
        let id = SceneEntityId(next_id);
        next_id += 1;
        document
            .scene
            .add_entity(id, format!("Root{index}"), None)
            .expect("fixture root must be valid");
        document
            .scene
            .entities
            .get_mut(&id)
            .expect("fixture root exists")
            .components
            .insert(
                "Transform".to_owned(),
                Transform3D {
                    position: Vec3::new(index as f32, 0.0, 0.0),
                    ..Transform3D::default()
                }
                .to_component_str(),
            );
        ids.push(id);
    }

    // Breadth-first fill over a parent queue: each parent receives at most `fan_out`
    // children before the next parent is used, so depth stays logarithmic
    // (about log_fanout(entities)) instead of degenerating into a deep chain that would
    // overflow the stack in recursive walks.
    let mut queue: std::collections::VecDeque<SceneEntityId> = ids.iter().copied().collect();
    let mut child_slots: Vec<usize> = vec![0; ids.len()];
    let mut ids_len = ids.len();
    while ids_len < entities {
        let Some(parent) = queue.pop_front() else {
            break;
        };
        let parent_index = (parent.0 - 1) as usize;
        let slots = child_slots.get(parent_index).copied().unwrap_or(0);
        if slots >= fan_out {
            continue;
        }
        let id = SceneEntityId(next_id);
        next_id += 1;
        document
            .scene
            .add_entity(id, format!("Node{}", id.0), Some(parent))
            .expect("fixture child must be valid");
        document
            .scene
            .entities
            .get_mut(&id)
            .expect("fixture child exists")
            .components
            .insert(
                "Transform".to_owned(),
                Transform3D {
                    position: Vec3::new(0.0, slots as f32, 0.0),
                    ..Transform3D::default()
                }
                .to_component_str(),
            );
        ids.push(id);
        ids_len += 1;
        if parent_index < child_slots.len() {
            child_slots[parent_index] = slots + 1;
        } else {
            child_slots.resize(parent_index + 1, 0);
            child_slots[parent_index] = slots + 1;
        }
        // Re-queue the parent while it still has free slots, then queue the new child.
        if slots + 1 < fan_out {
            queue.push_back(parent);
        }
        queue.push_back(id);
    }

    SceneFixture {
        expected_rows: ids.len(),
        document,
    }
}

/// Deterministic synthetic log lines with a mix of levels and a searchable token.
pub fn build_console_fixture(lines: usize) -> Vec<String> {
    (0..lines)
        .map(|index| match index % 10 {
            0 => format!("ERROR editor::fixture[{index}] failing_node_{index}"),
            3 => format!("WARN  editor::fixture[{index}] retrying_node_{index}"),
            7 => format!("INFO  editor::fixture[{index}] ready_node_{index}"),
            _ => format!("DEBUG editor::fixture[{index}] step_{index}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_fixture_has_exact_entity_count_and_roots() {
        let fixture = build_scene_fixture(FixtureSpec {
            entities: 10,
            children_per_node: 2,
            console_lines: 4,
        });
        assert_eq!(fixture.document.scene.entities.len(), 10);
        assert_eq!(fixture.expected_rows, 10);
        let roots = fixture
            .document
            .scene
            .entities
            .values()
            .filter(|entity| entity.parent.is_none())
            .count();
        assert_eq!(roots, 2);
    }

    #[test]
    fn large_fixture_matches_plan_target_size() {
        let fixture = build_scene_fixture(FixtureSize::Large.spec());
        assert_eq!(fixture.document.scene.entities.len(), 10_000);
        // Every non-root entity has a live parent and a transform component.
        for entity in fixture.document.scene.entities.values() {
            if let Some(parent) = entity.parent {
                assert!(fixture.document.scene.entities.contains_key(&parent));
                assert!(entity.components.contains_key("Transform"));
            }
        }
    }

    #[test]
    fn fixture_depth_stays_logarithmic() {
        // Guards the generator against degenerating into a chain: a 10k-entity fixture with
        // fan-out 5 must stay shallow, otherwise recursive walks overflow the stack on the
        // editor's main thread (1 MB) even though tests (8 MB) would still pass.
        for size in [FixtureSize::Small, FixtureSize::Medium, FixtureSize::Large] {
            let spec = size.spec();
            let fixture = build_scene_fixture(spec);
            let mut depth = 0_usize;
            for entity in fixture.document.scene.entities.values() {
                let mut current = entity.parent;
                let mut walked = 0_usize;
                while let Some(parent) = current {
                    walked += 1;
                    assert!(
                        walked <= fixture.document.scene.entities.len(),
                        "fixture tree contains a cycle"
                    );
                    current = fixture
                        .document
                        .scene
                        .entities
                        .get(&parent)
                        .and_then(|value| value.parent);
                }
                depth = depth.max(walked);
            }
            let max_allowed = 64;
            assert!(
                depth <= max_allowed,
                "{} fixture depth {depth} exceeds {max_allowed} (fan_out={})",
                size.label(),
                spec.children_per_node
            );
        }
    }

    #[test]
    fn fixture_ids_are_dense_and_stable() {
        let fixture = build_scene_fixture(FixtureSpec {
            entities: 32,
            children_per_node: 3,
            console_lines: 0,
        });
        let ids = fixture.entity_ids();
        assert_eq!(ids.len(), 32);
        assert_eq!(ids[0], SceneEntityId(1));
        assert_eq!(ids[31], SceneEntityId(32));
    }

    #[test]
    fn console_fixture_mixes_levels_and_token() {
        let lines = build_console_fixture(100);
        assert_eq!(lines.len(), 100);
        assert!(lines.iter().any(|line| line.starts_with("ERROR")));
        assert!(lines.iter().any(|line| line.contains("failing_node_0")));
    }
}
