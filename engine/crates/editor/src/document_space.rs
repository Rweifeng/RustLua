//! 编辑器的空间语义入口：实现已收进引擎（[`engine::core::scene_space`]），这里只做转发。
//!
//! 保留本模块是为了让编辑器的调用点（Scene 视图、层级面板）有一处稳定的名字，并在同一处说明
//! **为什么编辑态需要自己的入口**：编辑器在文档上工作（没有 ECS），而运行时在 World 上工作；
//! 两者的组合规则由引擎的 `propagate_all` / `propagate_spatial` 共用，且有等价性测试钉住。
//!
//! 组件串编码（`encode_transform_2d` / `encode_transform_3d`）也一并转发。

pub use engine::core::scene_space::{
    encode_transform_2d, encode_transform_3d, keep_world_after_reparent, local_keeping_world,
    local_transforms, locals_for_world_targets, parent_world_matrix, parents, world_matrices,
    world_transform_2d, world_transform_3d,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// 编辑器转发的是引擎实现：这里只钉住"名字还在、签名没变"，行为测试在
    /// `engine::core::scene_space` 里。
    #[test]
    fn editor_reexports_the_engine_document_space() {
        let mut scene = engine::core::scene::Scene::new("reexport");
        // docs/54 §4.1：断言的是父子组合，因此文档必须声明局部空间语义（未声明 ⇒ 组织关系）。
        scene.metadata.insert("spatial".into(), "local".into());
        let parent = engine::core::scene::SceneEntityId(1);
        let child = engine::core::scene::SceneEntityId(2);
        scene.add_entity(parent, "Parent", None).unwrap();
        scene.add_entity(child, "Child", Some(parent)).unwrap();
        for (id, x) in [(parent, 5.0_f32), (child, 2.0)] {
            scene.entities.get_mut(&id).unwrap().components.insert(
                "Transform3D".into(),
                format!("position={x},0,0;rotation=0,0,0,1;scale=1,1,1"),
            );
        }
        assert_eq!(
            world_transform_3d(&scene, child).expect("world").position.x,
            7.0
        );
        let mut targets = std::collections::BTreeMap::new();
        targets.insert(
            child,
            engine::core::spatial_transform::LocalTransform {
                translation: engine::core::math3d::Vec3::new(7.0, 0.0, 0.0),
                rotation: engine::core::math3d::Quat::identity(),
                scale: engine::core::math3d::Vec3::new(1.0, 1.0, 1.0),
            },
        );
        let locals = locals_for_world_targets(&scene, &targets);
        assert!((locals[&child].translation.x - 2.0).abs() < 1e-5);
    }
}
