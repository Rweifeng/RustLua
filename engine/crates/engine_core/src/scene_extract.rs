//! Shared extraction of serialized scene render components.

use crate::{
    camera3d::Camera3D,
    light::Light,
    math3d::Mat4,
    mesh3d::MeshRenderer,
    scene::{Scene, SceneEntityId},
    transform3d::Transform3D,
};
use std::{error::Error, fmt};

#[derive(Clone, Debug, PartialEq)]
pub struct Scene3dEntity {
    pub id: SceneEntityId,
    pub name: String,
    pub transform: Transform3D,
    pub model: Mat4,
    pub mesh: MeshRenderer,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SerializedScene3d {
    pub entities: Vec<Scene3dEntity>,
    pub cameras: Vec<(SceneEntityId, Camera3D)>,
    pub lights: Vec<(SceneEntityId, Light)>,
    pub diagnostics: Vec<SceneExtractDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneExtractDiagnostic {
    pub entity: SceneEntityId,
    pub name: String,
    pub component: String,
    pub message: String,
}

impl SceneExtractDiagnostic {
    #[must_use]
    pub fn summary(&self) -> String {
        format!("{} / {}：{}", self.name, self.component, self.message)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneExtractError {
    InvalidEntityComponent(SceneExtractDiagnostic),
}

impl fmt::Display for SceneExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEntityComponent(diagnostic) => write!(
                f,
                "实体 {} ({}) 的 {} 组件无效：{}",
                diagnostic.entity.0, diagnostic.name, diagnostic.component, diagnostic.message
            ),
        }
    }
}
impl Error for SceneExtractError {}

/// Extracts all valid serialized 3D render components while retaining errors.
pub fn extract_serialized_scene_3d(scene: &Scene) -> SerializedScene3d {
    let mut result = SerializedScene3d {
        entities: Vec::new(),
        cameras: Vec::new(),
        lights: Vec::new(),
        diagnostics: Vec::new(),
    };
    for entity in scene.entities.values() {
        let transform = match entity.components.get("Transform3D") {
            Some(value) => match Transform3D::from_component_str(value) {
                Ok(transform) => Some(transform),
                Err(error) => {
                    result.diagnostics.push(SceneExtractDiagnostic {
                        entity: entity.id,
                        name: entity.name.clone(),
                        component: "Transform3D".to_owned(),
                        message: error.to_string(),
                    });
                    None
                }
            },
            None => None,
        };
        if let Some(value) = entity.components.get("MeshRenderer") {
            match (transform, MeshRenderer::from_component_str(value)) {
                (Some(transform), Ok(mesh)) => match transform.model_matrix() {
                    Ok(model) => result.entities.push(Scene3dEntity {
                        id: entity.id,
                        name: entity.name.clone(),
                        transform,
                        model,
                        mesh,
                    }),
                    Err(error) => result.diagnostics.push(SceneExtractDiagnostic {
                        entity: entity.id,
                        name: entity.name.clone(),
                        component: "Transform3D".to_owned(),
                        message: error.to_string(),
                    }),
                },
                (_, Err(error)) => result.diagnostics.push(SceneExtractDiagnostic {
                    entity: entity.id,
                    name: entity.name.clone(),
                    component: "MeshRenderer".to_owned(),
                    message: error.to_string(),
                }),
                (None, _) => {}
            }
        }
        if let Some(value) = entity.components.get("Camera3D") {
            match Camera3D::from_component_str(value) {
                Ok(camera) => result.cameras.push((entity.id, camera)),
                Err(error) => result.diagnostics.push(SceneExtractDiagnostic {
                    entity: entity.id,
                    name: entity.name.clone(),
                    component: "Camera3D".to_owned(),
                    message: error.to_string(),
                }),
            }
        }
        if let Some(value) = entity.components.get("Light") {
            match Light::from_component_str(value) {
                Ok(light) => result.lights.push((entity.id, light)),
                Err(error) => result.diagnostics.push(SceneExtractDiagnostic {
                    entity: entity.id,
                    name: entity.name.clone(),
                    component: "Light".to_owned(),
                    message: error.to_string(),
                }),
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_renderable_and_reports_invalid_components() {
        let mut scene = Scene::new("3d");
        scene.add_entity(SceneEntityId(1), "Mesh", None).unwrap();
        scene.add_entity(SceneEntityId(2), "Broken", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform3D".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("MeshRenderer".into(), "mesh=assets/models/test.glb".into());
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("MeshRenderer".into(), "mesh=../escape.glb".into());
        let extracted = extract_serialized_scene_3d(&scene);
        assert_eq!(extracted.entities.len(), 1);
        assert_eq!(extracted.entities[0].id, SceneEntityId(1));
        assert_eq!(extracted.diagnostics.len(), 1);
        assert_eq!(extracted.diagnostics[0].entity, SceneEntityId(2));
    }
}
