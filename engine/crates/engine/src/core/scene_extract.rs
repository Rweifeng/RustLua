//! Shared extraction of serialized scene render components.

use crate::core::{
    camera::Camera3D,
    light::Light,
    math3d::Mat4,
    mesh3d::MeshRenderer,
    scene::{Scene, SceneEntityId},
    transform::Transform3D,
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
                Ok(camera) => {
                    // WP5 / docs/54 §8-⑦：`pose=transform` 的相机姿态取实体的**世界**变换
                    // （父链组合后的结果），而不是局部值——相机挂在父下时才算对得上画面。
                    // 取不到就**报诊断**并保持组件姿态；沉默地"看起来没生效"是最难查的一类问题。
                    let camera = if camera.pose_from_transform {
                        match crate::core::scene_space::world_transform_3d(scene, entity.id) {
                            Some(pose) => match camera.with_pose(pose) {
                                Ok(posed) => posed,
                                Err(error) => {
                                    result.diagnostics.push(SceneExtractDiagnostic {
                                        entity: entity.id,
                                        name: entity.name.clone(),
                                        component: "Camera3D".to_owned(),
                                        message: format!(
                                            "pose=transform 无法应用（{error}），已回退到组件姿态"
                                        ),
                                    });
                                    camera
                                }
                            },
                            None => {
                                result.diagnostics.push(SceneExtractDiagnostic {
                                    entity: entity.id,
                                    name: entity.name.clone(),
                                    component: "Camera3D".to_owned(),
                                    message: "Camera3D 声明了 pose=transform，但该实体没有可用的 Transform3D（或无法无损分解），已回退到组件姿态"
                                        .to_owned(),
                                });
                                camera
                            }
                        }
                    } else {
                        camera
                    };
                    result.cameras.push((entity.id, camera));
                }
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

    fn game_root() -> std::path::PathBuf {
        crate::core::games_dir().join("examples").join("game_api")
    }

    /// 稳定快照：对 `demo3d.scene` 的实体名称、mesh path、transform、camera/light
    /// 建立固定断言，防止序列化 extractor 意外改变提取结果。
    #[test]
    fn demo3d_scene_stable_snapshot() {
        let root = game_root();
        let path = root.join("scenes/demo3d.scene");
        if !path.is_file() {
            return;
        }
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load demo3d: {e}"));
        let extracted = extract_serialized_scene_3d(&scene);

        // 实体名称 / mesh path / 数量快照。
        let names: Vec<&str> = extracted.entities.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["ShadowTriangle", "Ground", "SkeletonWarrior"],
            "实体顺序与名称（BTreeMap 按 id 排序）应稳定"
        );
        assert_eq!(
            extracted.entities[0].mesh.mesh,
            "assets/models/m8-test-triangle.glb"
        );
        assert_eq!(
            extracted.entities[1].mesh.mesh,
            crate::core::mesh3d::GROUND_PLANE_SENTINEL
        );
        assert_eq!(
            extracted.entities[2].mesh.mesh,
            "assets/models/kaykit-warrior.glb"
        );

        // Transform：SkeletonWarrior 位于 (1.5, 0, 0)，模型矩阵第 4 列为平移（列主序）。
        let warrior = &extracted.entities[2];
        assert_eq!(
            warrior.transform.position,
            crate::core::math3d::Vec3::new(1.5, 0.0, 0.0)
        );
        let translation = [
            warrior.model.columns[3][0],
            warrior.model.columns[3][1],
            warrior.model.columns[3][2],
        ];
        assert!(
            translation
                .iter()
                .zip([1.5, 0.0, 0.0])
                .all(|(actual, expected)| (actual - expected).abs() < 1e-5),
            "模型矩阵平移应等于位置: {translation:?}"
        );

        // Camera：两个串行 Camera3D，aspect 默认 1.0（fov 存弧度）。
        assert_eq!(extracted.cameras.len(), 2);
        let (first_id, first_camera) = &extracted.cameras[0];
        assert_eq!(*first_id, crate::core::scene::SceneEntityId(1));
        assert_eq!(
            first_camera.position,
            crate::core::math3d::Vec3::new(0.0, 0.0, 6.0)
        );
        assert!((first_camera.fov_y - 60f32.to_radians()).abs() < 1e-5);
        assert!((first_camera.aspect_ratio - 1.0).abs() < 1e-6);

        // Light：唯一方向光。`direction` 是**光传播方向**，太阳因此是 y 为负（自上而下）——
        // 快照在这里钉住这个符号：写反会导致朝上的表面 `diffuse = 0`、场景只剩环境光，
        // 而这类错误在"有内容/无量纲断言"的 golden 里看不出来（docs/54 §10 第 ⑩ 行记着这次教训）。
        assert_eq!(extracted.lights.len(), 1);
        assert_eq!(extracted.lights[0].0, crate::core::scene::SceneEntityId(3));
        assert_eq!(
            extracted.lights[0].1.direction,
            crate::core::math3d::Vec3::new(0.4, -0.8, 0.5)
        );
        assert!(
            extracted.lights[0].1.direction.y < 0.0,
            "太阳必须是自上而下照（y < 0）：direction 是光传播方向"
        );

        // 合法场景不应有提取诊断。
        assert!(extracted.diagnostics.is_empty(), "demo3d 应无诊断");
    }

    /// “无 drawable”与“提取失败”不混为一谈：缺少 Transform3D 的 MeshRenderer
    /// 是静默跳过（无实体、无诊断）；非法组件字符串才产生带实体/名称/组件的诊断。
    #[test]
    fn missing_transform_is_not_extraction_failure() {
        let mut scene = Scene::new("distinguish");
        scene
            .add_entity(SceneEntityId(1), "NoTransform", None)
            .unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("MeshRenderer".into(), "mesh=assets/models/test.glb".into());
        scene
            .add_entity(SceneEntityId(2), "BrokenMesh", None)
            .unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Transform3D".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("MeshRenderer".into(), "mesh=../escape.glb".into());
        let extracted = extract_serialized_scene_3d(&scene);
        // 缺 Transform3D → 无 drawable 也无需诊断；坏 mesh → 明确诊断。
        assert_eq!(extracted.entities.len(), 0);
        assert_eq!(extracted.diagnostics.len(), 1);
        assert_eq!(extracted.diagnostics[0].entity, SceneEntityId(2));
        assert_eq!(extracted.diagnostics[0].name, "BrokenMesh");
        assert_eq!(extracted.diagnostics[0].component, "MeshRenderer");
        assert!(!extracted.diagnostics[0].message.is_empty());
    }

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

    /// WP5：`pose=transform` 的相机，姿态取实体的 `Transform3D`；取不到时**必须报诊断**
    /// 并回退到组件姿态——沉默地"看起来没生效"是最难查的一类问题。
    #[test]
    fn camera_pose_from_transform_is_extracted_and_diagnosed_when_missing() {
        let mut scene = Scene::new("3d");
        // ① 同时有 Transform3D 与 Camera3D(pose=transform)：位置取变换位置。
        scene.add_entity(SceneEntityId(1), "Posed", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform3D".into(),
                "position=7,8,9;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Camera3D".into(),
                "position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100;pose=transform".into(),
            );
        // ② 声明了 pose=transform 但没有 Transform3D：必须报诊断。
        scene.add_entity(SceneEntityId(2), "Unposed", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Camera3D".into(),
                "position=1,2,3;target=0,0,0;fov=60;near=0.1;far=100;pose=transform".into(),
            );

        let extracted = extract_serialized_scene_3d(&scene);
        assert_eq!(extracted.cameras.len(), 2);
        let posed = extracted
            .cameras
            .iter()
            .find(|(id, _)| *id == SceneEntityId(1))
            .map(|(_, camera)| *camera)
            .expect("posed camera");
        assert!((posed.position.x - 7.0).abs() < 1e-4);
        assert!((posed.position.y - 8.0).abs() < 1e-4);
        assert!((posed.position.z - 9.0).abs() < 1e-4);
        assert!(posed.pose_from_transform);

        let unposed = extracted
            .cameras
            .iter()
            .find(|(id, _)| *id == SceneEntityId(2))
            .map(|(_, camera)| *camera)
            .expect("unposed camera");
        assert!(
            (unposed.position.z - 3.0).abs() < 1e-4,
            "没有 Transform3D 时保持组件姿态"
        );
        assert_eq!(extracted.diagnostics.len(), 1);
        assert_eq!(extracted.diagnostics[0].entity, SceneEntityId(2));
        assert!(
            extracted.diagnostics[0].message.contains("pose=transform"),
            "诊断必须点名 pose=transform：{}",
            extracted.diagnostics[0].message
        );
    }
}
