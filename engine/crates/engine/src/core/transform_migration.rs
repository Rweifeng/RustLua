//! 变换组件的**统一迁移**：编辑器只保留一个 `Transform` 组件（3D 表示）。
//!
//! 历史上同一个概念有两种写法：
//! - 组件名 `Transform3D` + `position=x,y,z;rotation=rx,ry,rz,rw;scale=sx,sy,sz`
//!   ——这就是统一后的表示，迁移只需**改名 + 规范化数字格式**；
//! - 组件名 `Transform` + `x=..;y=..;rotation=..;scale_x=..;scale_y=..`（2D）
//!   ——升级成 `position=(x, y, 0)`、`rotation=绕 Z 轴的四元数`、`scale=(sx, sy, 1)`。
//!
//! 2D→3D 的换算**复用** [`LocalTransform::from_2d`]（渲染路径本来就用它），不另写一份，
//! 否则两处对"2D 旋转是绕哪个轴、什么方向"的理解迟早分叉。
//!
//! 迁移只改**内存里的场景**：磁盘文件在用户保存前一字不动。所以这里只有纯函数 + 一个场景级入口，
//! 标记未保存由调用方负责。（同一纪律曾被 `LuaScript`→`Script` 的迁移使用，那一层已在第 55 轮
//! 按用户要求删除；本模块与 `camera_migration` 是把它当作**一次性数据迁移工具**保留的最后两处，
//! 等示例数据迁完、旧名删除后一并移除。）

use crate::core::{
    scene::{Scene, SceneEntityId},
    spatial_transform::LocalTransform,
    transform::Transform3D,
};

/// 统一后的组件名。
pub const TRANSFORM_COMPONENT: &str = "Transform";
/// 旧的 3D 组件名（迁移后不再写出）。
pub const LEGACY_TRANSFORM_3D_COMPONENT: &str = "Transform3D";

/// 一次场景级迁移的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransformMigrationOutcome {
    /// 被改动的实体数（改名 / 2D 升级 / 合并都算一条）。
    pub changed_entities: usize,
    /// **没有改动**的实体：组件串解析失败。`(实体, 组件名, 原因)`。
    ///
    /// 解析不了就原样留着并报出来——宁可让作者在审计里看见一个坏组件，
    /// 也不猜着删掉或改写成一份"看起来能跑"的数据。
    pub skipped: Vec<(SceneEntityId, String, String)>,
}

impl TransformMigrationOutcome {
    /// 迁移是否真的改动了场景（调用方据此决定要不要标记未保存）。
    #[must_use]
    pub const fn changed(&self) -> bool {
        self.changed_entities > 0
    }
}

/// 把**一个**变换组件归一成统一写法。
///
/// - `Ok(None)`：已经是统一写法（不必改动）；
/// - `Ok(Some(payload))`：迁移后的组件串（调用方写回 `Transform` 键，并删掉旧键）；
/// - `Err(reason)`：解析失败（调用方原样保留）。
pub fn unified_transform_payload(name: &str, payload: &str) -> Result<Option<String>, String> {
    match name {
        LEGACY_TRANSFORM_3D_COMPONENT => {
            let transform =
                Transform3D::from_component_str(payload).map_err(|error| error.to_string())?;
            let canonical = transform.to_component_str();
            Ok((canonical != payload).then_some(canonical))
        }
        TRANSFORM_COMPONENT => {
            // 统一读取器负责"3D 写法 / 2D 旧写法"两种历史形态（与运行时读侧同一份实现）。
            let local = LocalTransform::from_unified_component_str(payload)?;
            let canonical = local.to_unified_component_str();
            Ok((canonical != payload).then_some(canonical))
        }
        _ => Ok(None),
    }
}

/// 迁移整份场景：`Transform3D` 改名成 `Transform`、2D 旧串升级成统一表示。
///
/// **同时存在两者时以 `Transform3D` 为准**（它是显式的 3D 数据），2D 的旧键被丢弃；
/// 这一条在测试里钉住，避免"合并策略"变成隐式推断。
pub fn migrate_scene_transforms(scene: &mut Scene) -> TransformMigrationOutcome {
    let mut outcome = TransformMigrationOutcome::default();
    for entity in scene.entities.values_mut() {
        let legacy_3d = entity
            .components
            .get(LEGACY_TRANSFORM_3D_COMPONENT)
            .cloned();
        if let Some(payload) = legacy_3d {
            match unified_transform_payload(LEGACY_TRANSFORM_3D_COMPONENT, &payload) {
                Ok(Some(next)) => {
                    entity
                        .components
                        .insert(TRANSFORM_COMPONENT.to_owned(), next);
                    entity.components.remove(LEGACY_TRANSFORM_3D_COMPONENT);
                    outcome.changed_entities += 1;
                }
                Ok(None) => {
                    // 规范化后与原文一致：仍要改名（把值搬过去）。
                    entity
                        .components
                        .insert(TRANSFORM_COMPONENT.to_owned(), payload);
                    entity.components.remove(LEGACY_TRANSFORM_3D_COMPONENT);
                    outcome.changed_entities += 1;
                }
                Err(reason) => {
                    outcome
                        .skipped
                        .push((entity.id, LEGACY_TRANSFORM_3D_COMPONENT.to_owned(), reason));
                }
            }
            // 显式的 3D 数据优先：无论改名成功与否，都不再对同实体的 2D 串做升级，
            // 免得把"坏掉的 3D 数据"悄悄替换成一份 2D 升级结果。
            continue;
        }
        let Some(payload) = entity.components.get(TRANSFORM_COMPONENT).cloned() else {
            continue;
        };
        match unified_transform_payload(TRANSFORM_COMPONENT, &payload) {
            Ok(Some(next)) => {
                entity.components.insert(TRANSFORM_COMPONENT.to_owned(), next);
                outcome.changed_entities += 1;
            }
            Ok(None) => {}
            Err(reason) => outcome
                .skipped
                .push((entity.id, TRANSFORM_COMPONENT.to_owned(), reason)),
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scene::Scene;

    fn scene_with(components: &[(&str, &str)]) -> (Scene, SceneEntityId) {
        let mut scene = Scene::new("migration");
        let id = SceneEntityId(1);
        scene.add_entity(id, "Hero", None).unwrap();
        for (name, payload) in components {
            scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert((*name).to_owned(), (*payload).to_owned());
        }
        (scene, id)
    }

    /// 2D 旧串升级成统一表示：`position=(x,y,0)`、绕 Z 的四元数、`scale=(sx,sy,1)`。
    #[test]
    fn legacy_2d_transform_is_upgraded_to_the_unified_form() {
        let payload = "x=3.000000000;y=-4.000000000;rotation=1.570796327;scale_x=2.000000000;scale_y=0.500000000";
        let migrated = unified_transform_payload(TRANSFORM_COMPONENT, payload)
            .expect("可解析")
            .expect("需要迁移");
        let parsed = Transform3D::from_component_str(&migrated).expect("迁移结果是合法 3D 串");
        assert_eq!(parsed.position, crate::core::math3d::Vec3::new(3.0, -4.0, 0.0));
        assert_eq!(parsed.scale, crate::core::math3d::Vec3::new(2.0, 0.5, 1.0));
        // 绕 Z 转 90°：X 轴映射到 +Y。
        let rotated = parsed
            .rotation
            .rotate(crate::core::math3d::Vec3::X)
            .expect("旋转");
        assert!(rotated.y > 0.999, "2D rotation 是绕 +Z 的逆时针角：{rotated:?}");
        // 与渲染路径用的换算同源（`LocalTransform::from_2d`）。
        let legacy = crate::core::transform::Transform::from_component_str(payload).unwrap();
        let expected = LocalTransform::from_2d(legacy).unwrap();
        assert_eq!(parsed.position, expected.translation);
        assert_eq!(parsed.rotation, expected.rotation);
        assert_eq!(parsed.scale, expected.scale);
    }

    /// 已经是统一写法 → 返回 `None`（幂等：迁移跑第二遍必须 0 改动）。
    #[test]
    fn unified_payload_is_idempotent() {
        let canonical = Transform3D {
            position: crate::core::math3d::Vec3::new(1.0, 2.0, 3.0),
            rotation: crate::core::math3d::Quat::identity(),
            scale: crate::core::math3d::Vec3::new(1.0, 1.0, 1.0),
        }
        .to_component_str();
        assert_eq!(
            unified_transform_payload(TRANSFORM_COMPONENT, &canonical),
            Ok(None)
        );
        // `Transform3D` 名的同一份串即使内容规范，也要迁移（因为要改名）。
        assert_eq!(
            unified_transform_payload(LEGACY_TRANSFORM_3D_COMPONENT, &canonical),
            Ok(None),
            "内容已规范 → 内容层无需改写"
        );
        let (mut scene, id) = scene_with(&[(LEGACY_TRANSFORM_3D_COMPONENT, &canonical)]);
        let outcome = migrate_scene_transforms(&mut scene);
        assert_eq!(outcome.changed_entities, 1, "改名算一次改动");
        assert!(outcome.skipped.is_empty());
        let components = &scene.entities[&id].components;
        assert!(!components.contains_key(LEGACY_TRANSFORM_3D_COMPONENT));
        assert_eq!(components[TRANSFORM_COMPONENT], canonical);
        // 第二遍：已经是统一写法 → 0 改动（幂等，不会让文档永远 dirty）。
        let outcome = migrate_scene_transforms(&mut scene);
        assert_eq!(outcome.changed_entities, 0);
        assert!(!outcome.changed());
    }

    /// 同一实体两种写法并存：以显式 3D 为准，2D 旧值被丢弃（合并策略是明写的，不靠推断）。
    #[test]
    fn explicit_3d_wins_when_both_forms_are_present() {
        let three_d = Transform3D {
            position: crate::core::math3d::Vec3::new(7.0, 0.0, 0.0),
            rotation: crate::core::math3d::Quat::identity(),
            scale: crate::core::math3d::Vec3::new(1.0, 1.0, 1.0),
        }
        .to_component_str();
        let (mut scene, id) = scene_with(&[
            ("Transform", "x=1.000000000;y=1.000000000;rotation=0.000000000;scale_x=1.000000000;scale_y=1.000000000"),
            (LEGACY_TRANSFORM_3D_COMPONENT, &three_d),
        ]);
        let outcome = migrate_scene_transforms(&mut scene);
        assert_eq!(outcome.changed_entities, 1);
        let components = &scene.entities[&id].components;
        assert_eq!(components[TRANSFORM_COMPONENT], three_d);
        assert!(!components.contains_key(LEGACY_TRANSFORM_3D_COMPONENT));
    }

    /// 坏数据**原样保留**并进 `skipped`：既不猜着删，也不改写成别的东西。
    #[test]
    fn unparsable_payloads_are_left_untouched_and_reported() {
        let (mut scene, id) = scene_with(&[(LEGACY_TRANSFORM_3D_COMPONENT, "position=oops")]);
        let outcome = migrate_scene_transforms(&mut scene);
        assert_eq!(outcome.changed_entities, 0);
        assert_eq!(outcome.skipped.len(), 1);
        assert_eq!(outcome.skipped[0].0, id);
        assert_eq!(outcome.skipped[0].1, LEGACY_TRANSFORM_3D_COMPONENT);
        let components = &scene.entities[&id].components;
        assert_eq!(components[LEGACY_TRANSFORM_3D_COMPONENT], "position=oops");
        assert!(!components.contains_key(TRANSFORM_COMPONENT));

        // 2D 名下的坏串同样原样保留。
        let (mut scene, id) = scene_with(&[("Transform", "x=oops")]);
        let outcome = migrate_scene_transforms(&mut scene);
        assert_eq!(outcome.changed_entities, 0);
        assert_eq!(outcome.skipped.len(), 1);
        assert_eq!(scene.entities[&id].components["Transform"], "x=oops");
    }

    /// 完全没有变换组件的实体、以及非变换组件都不受影响。
    #[test]
    fn unrelated_components_and_entities_are_ignored() {
        let (mut scene, id) = scene_with(&[
            ("Sprite", "texture=;size=1x1;tint=1,1,1,1;sorting_layer=default"),
            ("Light", "direction=0,1,0;color=1,1,1;intensity=1"),
        ]);
        let outcome = migrate_scene_transforms(&mut scene);
        assert_eq!(outcome.changed_entities, 0);
        assert!(outcome.skipped.is_empty());
        assert_eq!(
            scene.entities[&id].components["Light"],
            "direction=0,1,0;color=1,1,1;intensity=1"
        );
        assert_eq!(
            unified_transform_payload("Sprite", "texture=;size=1x1"),
            Ok(None)
        );
    }
}
