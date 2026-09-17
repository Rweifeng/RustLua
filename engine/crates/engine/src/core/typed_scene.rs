//! WP1 prototype: compile the human-readable v2 scene into typed runtime data.

use std::{collections::BTreeMap, fmt};

use crate::core::{
    camera3d::Camera3D,
    light::Light,
    mesh3d::MeshRenderer,
    resource_db::AssetId,
    scene::{Scene, SceneEntityId},
    transform::Transform,
    transform3d::Transform3D,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ComponentId {
    Transform2D,
    Transform3D,
    Camera3D,
    Light,
    MeshRenderer,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypedComponent {
    Transform2D(Transform),
    Transform3D(Transform3D),
    Camera3D(Camera3D),
    Light(Light),
    MeshRenderer {
        renderer: MeshRenderer,
        asset: AssetId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypedEntity {
    pub id: SceneEntityId,
    pub name: String,
    pub parent: Option<SceneEntityId>,
    pub components: BTreeMap<ComponentId, TypedComponent>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypedScene {
    pub source_version: u32,
    pub name: String,
    pub entities: Vec<TypedEntity>,
    pub parent_first: Vec<SceneEntityId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedSceneDiagnostic {
    pub entity: SceneEntityId,
    pub name: String,
    pub component: String,
    pub message: String,
}

impl fmt::Display for TypedSceneDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "entity {} ({}) component {}: {}",
            self.entity.0, self.name, self.component, self.message
        )
    }
}

impl std::error::Error for TypedSceneDiagnostic {}

impl TypedScene {
    /// Compiles known v2 components once at the load boundary. Unknown/dynamic
    /// components stay in the source scene and are deliberately not interpreted.
    pub fn compile_v2(scene: &Scene) -> Result<Self, Vec<TypedSceneDiagnostic>> {
        let mut diagnostics = Vec::new();
        let mut entities = Vec::with_capacity(scene.entities.len());
        for source in scene.entities.values() {
            let mut components = BTreeMap::new();
            for (name, value) in &source.components {
                let parsed = match name.as_str() {
                    "Transform" => Transform::from_component_str(value)
                        .map(TypedComponent::Transform2D)
                        .map(|value| (ComponentId::Transform2D, value))
                        .map_err(|error| error.to_string()),
                    "Transform3D" => Transform3D::from_component_str(value)
                        .map(TypedComponent::Transform3D)
                        .map(|value| (ComponentId::Transform3D, value))
                        .map_err(|error| error.to_string()),
                    "Camera3D" => Camera3D::from_component_str(value)
                        .map(TypedComponent::Camera3D)
                        .map(|value| (ComponentId::Camera3D, value))
                        .map_err(|error| error.to_string()),
                    "Light" => Light::from_component_str(value)
                        .map(TypedComponent::Light)
                        .map(|value| (ComponentId::Light, value))
                        .map_err(|error| error.to_string()),
                    "MeshRenderer" => MeshRenderer::from_component_str(value)
                        .map(|renderer| {
                            let asset = AssetId::from_seed(&renderer.mesh);
                            (
                                ComponentId::MeshRenderer,
                                TypedComponent::MeshRenderer { renderer, asset },
                            )
                        })
                        .map_err(|error| error.to_string()),
                    _ => continue,
                };
                match parsed {
                    Ok((id, component)) => {
                        components.insert(id, component);
                    }
                    Err(message) => diagnostics.push(TypedSceneDiagnostic {
                        entity: source.id,
                        name: source.name.clone(),
                        component: name.clone(),
                        message,
                    }),
                }
            }
            entities.push(TypedEntity {
                id: source.id,
                name: source.name.clone(),
                parent: source.parent,
                components,
            });
        }
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        let mut parent_first = Vec::with_capacity(entities.len());
        let by_id = entities
            .iter()
            .map(|entity| (entity.id, entity))
            .collect::<BTreeMap<_, _>>();
        let mut states = BTreeMap::new();
        fn visit(
            id: SceneEntityId,
            by_id: &BTreeMap<SceneEntityId, &TypedEntity>,
            states: &mut BTreeMap<SceneEntityId, u8>,
            order: &mut Vec<SceneEntityId>,
        ) -> Result<(), TypedSceneDiagnostic> {
            match states.get(&id).copied() {
                Some(2) => return Ok(()),
                Some(1) => {
                    let entity = by_id[&id];
                    return Err(TypedSceneDiagnostic {
                        entity: id,
                        name: entity.name.clone(),
                        component: "parent".to_owned(),
                        message: "parent cycle".to_owned(),
                    });
                }
                _ => {}
            }
            states.insert(id, 1);
            let entity = by_id[&id];
            if let Some(parent) = entity.parent {
                if !by_id.contains_key(&parent) {
                    return Err(TypedSceneDiagnostic {
                        entity: id,
                        name: entity.name.clone(),
                        component: "parent".to_owned(),
                        message: format!("missing parent {}", parent.0),
                    });
                }
                visit(parent, by_id, states, order)?;
            }
            states.insert(id, 2);
            order.push(id);
            Ok(())
        }
        for id in by_id.keys().copied() {
            if let Err(error) = visit(id, &by_id, &mut states, &mut parent_first) {
                return Err(vec![error]);
            }
        }
        Ok(Self {
            source_version: scene.version,
            name: scene.name.clone(),
            entities,
            parent_first,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_known_components_and_parent_topology_once() {
        let mut scene = Scene::new("typed");
        scene.add_entity(SceneEntityId(1), "Parent", None).unwrap();
        scene
            .add_entity(SceneEntityId(2), "Child", Some(SceneEntityId(1)))
            .unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Transform3D".into(),
                "position=1,2,3;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "MeshRenderer".into(),
                "mesh=assets/models/m8-test-triangle.glb".into(),
            );
        let typed = TypedScene::compile_v2(&scene).unwrap();
        assert_eq!(typed.parent_first, [SceneEntityId(1), SceneEntityId(2)]);
        let child = typed
            .entities
            .iter()
            .find(|entity| entity.id == SceneEntityId(2))
            .unwrap();
        assert!(child.components.contains_key(&ComponentId::Transform3D));
        assert!(child.components.contains_key(&ComponentId::MeshRenderer));
    }

    #[test]
    fn diagnostics_identify_entity_component_and_error() {
        let mut scene = Scene::new("invalid");
        scene.add_entity(SceneEntityId(7), "Broken", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(7))
            .unwrap()
            .components
            .insert(
                "Transform3D".into(),
                "position=bad;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        let error = TypedScene::compile_v2(&scene).unwrap_err();
        assert_eq!(error[0].entity, SceneEntityId(7));
        assert_eq!(error[0].component, "Transform3D");
        assert!(error[0].to_string().contains("Broken"));
    }
}
