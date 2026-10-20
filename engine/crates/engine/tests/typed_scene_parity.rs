//! WP1-3 对拍门：把项目里**全部真实源场景**编译一遍，并与旧提取路径逐项对拍。
//!
//! ## 为什么这条门在 `engine` 而不是 `rustlua-scene`（`docs/62` P1 ④）
//!
//! 它要同时看见两侧：`rustlua_scene::typed_scene`（文本 → 类型化组件）与
//! `extract_serialized_scene_3d`（文本 → 渲染层类型）。后者产出 `MeshRenderer`/`Light`/
//! `Camera3D`，文法与类型都在 `rustlua-render`/`rustlua-camera` 里。P1 ④ 之后 scene
//! 不再依赖 `render`，旧提取整块搬到了 `engine::host::scene_extract`——于是这条对拍门
//! 也跟着搬到组合根，成为**唯一同时看得见两侧**的地方（和 `component_schema_parity` 同理）。
//!
//! 断言三件事：① 每个场景都能无诊断编译；② 场景中的已知组件数与源组件数一致
//! （若某个组件名未被 typed 覆盖，这里会差出来——覆盖缺口必须补，不能跳过）；
//! ③ 3D drawable 与旧提取的数量、名称、模型矩阵、mesh 路径以及相机/光源集合完全一致。
//!
//! 场景目录缺失时**不静默通过**：本门的意义就是真的跑过全部工程场景。

use std::collections::BTreeMap;

use engine::host::scene_extract::extract_serialized_scene_3d;
use rustlua_camera::camera::CameraProjection;
use rustlua_math::math3d::Mat4;
use rustlua_scene::scene::{Scene, SceneEntityId};
use rustlua_scene::typed_scene::{ComponentId, TypedComponent, TypedScene};

/// 矩阵逐元素比较（旧路径与 typed 路径的浮点顺序可能不同，只比对到 1e-5）。
fn matrices_match(left: Mat4, right: Mat4) -> bool {
    left.columns
        .iter()
        .flatten()
        .zip(right.columns.iter().flatten())
        .all(|(a, b)| (a - b).abs() < 1e-5)
}

#[test]
fn compiles_every_project_scene_and_matches_serialized_extraction() {
    // 表现层组件的 codec 由组合根装表（P1 ④）：本门要编译真实场景文本，必须先装。
    engine::host::scene_codecs::install();

    let roots = [
        rustlua_infra::games_dir().join("examples/game_api/scenes"),
        rustlua_infra::games_dir().join("examples/game_jhyt/scenes"),
    ];
    let mut scene_count = 0;
    for root in roots {
        let entries = std::fs::read_dir(&root)
            .unwrap_or_else(|error| panic!("读取场景目录失败 {}: {error}", root.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("scene") {
                continue;
            }
            let label = path.display().to_string();
            let scene = Scene::load(&path).unwrap_or_else(|error| panic!("{label}: {error}"));
            scene_count += 1;

            // ① 无诊断编译。
            let typed = TypedScene::compile_v2(&scene)
                .unwrap_or_else(|errors| panic!("{label} 编译诊断：{errors:?}"));

            // ② 覆盖完整性：源组件总数必须与 typed 组件总数相等。
            let source_components: usize = scene
                .entities
                .values()
                .map(|entity| entity.components.len())
                .sum();
            let typed_components: usize = typed
                .entities
                .iter()
                .map(|entity| entity.components.len())
                .sum();
            assert_eq!(
                typed_components, source_components,
                "{label} 存在未被 typed 覆盖的组件名（源 {source_components} ≠ typed {typed_components}）"
            );

            // ③ 3D 对拍。
            let serialized = extract_serialized_scene_3d(&scene);
            assert!(
                serialized.diagnostics.is_empty(),
                "{label} 旧提取产生诊断：{:?}",
                serialized.diagnostics
            );
            let old_3d: BTreeMap<&str, (Mat4, &str)> = serialized
                .entities
                .iter()
                .map(|entity| {
                    (
                        entity.name.as_str(),
                        (entity.model, entity.mesh.mesh.as_str()),
                    )
                })
                .collect();
            let typed_3d: BTreeMap<&str, (Mat4, &str)> = typed
                .entities
                .iter()
                .filter_map(|entity| {
                    let Some(TypedComponent::Transform(transform)) =
                        entity.components.get(&ComponentId::Transform)
                    else {
                        return None;
                    };
                    let Some(TypedComponent::Opaque { assets, .. }) =
                        entity.components.get(&ComponentId::MeshRenderer)
                    else {
                        return None;
                    };
                    // 与旧提取一致：非法变换的实体在旧路径会变成诊断而不是 drawable，
                    // 上面的诊断断言已保证这里不会静默丢实体。
                    let model = transform.model_matrix().ok()?;
                    // `MeshRenderer` 的 mesh 路径由 render 的 codec 报告，typed 侧只做搬运。
                    let mesh = assets.first()?.0.as_str();
                    Some((entity.name.as_str(), (model, mesh)))
                })
                .collect();
            assert_eq!(
                typed_3d.len(),
                old_3d.len(),
                "{label} 新旧 3D drawable 数量不一致（typed {} ≠ 旧 {}）",
                typed_3d.len(),
                old_3d.len()
            );
            assert_eq!(
                typed_3d.len(),
                typed
                    .entities
                    .iter()
                    .filter(|entity| {
                        entity.components.contains_key(&ComponentId::Transform)
                            && entity.components.contains_key(&ComponentId::MeshRenderer)
                    })
                    .count(),
                "{label} 3D 实体名重复，无法逐名对拍"
            );
            for (name, (model, mesh)) in &old_3d {
                let (typed_model, typed_mesh) = typed_3d
                    .get(name)
                    .unwrap_or_else(|| panic!("{label} typed 缺少 3D 实体 {name}"));
                assert!(
                    matrices_match(*model, *typed_model),
                    "{label} {name} 模型矩阵不一致"
                );
                assert_eq!(*mesh, *typed_mesh, "{label} {name} mesh 路径不一致");
            }

            // 相机与光源集合必须一致（旧路径按同一实体顺序收集）。
            let old_cameras: Vec<SceneEntityId> = serialized
                .cameras
                .iter()
                .map(|(entity, _)| *entity)
                .collect();
            let typed_cameras: Vec<SceneEntityId> = typed
                .entities
                .iter()
                .filter(|entity| {
                    matches!(
                        entity.components.get(&ComponentId::Camera),
                        Some(TypedComponent::Camera(camera))
                            if camera.projection() == CameraProjection::Perspective
                    )
                })
                .map(|entity| entity.id)
                .collect();
            assert_eq!(typed_cameras, old_cameras, "{label} 透视相机集合不一致");
            let old_lights: Vec<SceneEntityId> = serialized
                .lights
                .iter()
                .map(|(entity, _)| *entity)
                .collect();
            let typed_lights: Vec<SceneEntityId> = typed
                .entities
                .iter()
                .filter(|entity| entity.components.contains_key(&ComponentId::Light))
                .map(|entity| entity.id)
                .collect();
            assert_eq!(typed_lights, old_lights, "{label} 光源集合不一致");
        }
    }
    assert!(scene_count > 0, "没有找到任何源场景，对拍门实际未执行");
}
