//! WP2：实体的空间父子关系与世界变换传播。
//!
//! 负责人裁定"所有场景的 `parent` 一律按**局部空间**解释"。落地需要三件事：
//! ① 运行时知道父子关系（[`Parent`] 组件）；② 每帧把局部变换沿父链组合成**世界**变换
//! （[`propagate_spatial`]，结果写进 [`WorldTransform`] / [`WorldTransform3D`]）；③ 消费者
//! 优先读世界变换、没有时退回局部（[`world_transform_2d`] / [`world_transform_3d`]）。
//!
//! 组合走矩阵再分解，而不是手写 TRS 公式：非均匀父缩放与子旋转的组合会产生**剪切**，那不是
//! TRS 能表达的东西。分解失败时**保留局部值当世界值**并记一条诊断，绝不近似——近似会让子实体
//! 悄悄变形或位移。

use crate::core::{
    entity::EntityId,
    math3d::Mat4,
    spatial_transform::{DecomposeError, LocalTransform},
    transform::{Transform, Transform3D},
    world::{World, WorldError},
};

/// 实体的空间父节点（运行时组件）。场景里的 `parent` 在实例化时落到这里。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Parent(pub EntityId);

/// 传播算出来的世界变换（2D 空间）。**只有开启空间继承时才会存在**——继承关闭时消费者退回
/// 局部 `Transform`，因此关掉开关就是"逐字节回到旧行为"的回滚档。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldTransform(pub Transform);

/// 传播算出来的世界变换（3D 空间）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldTransform3D(pub Transform3D);

/// 一个没能建立空间继承的实体：保留局部值当世界值。
#[derive(Clone, Debug, PartialEq)]
pub struct SpatialDiagnostic {
    pub entity: EntityId,
    pub reason: SpatialDiagnosticReason,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SpatialDiagnosticReason {
    /// 父链成环（含自环）：整条链上的实体都保留局部值。
    Cycle,
    /// 局部或组合结果无法无损表达为 TRS（剪切/镜像/退化/奇异）。
    NotRepresentable(DecomposeError),
}

/// 传播结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpatialReport {
    /// 本帧算出世界变换的实体数（不含保留局部值的那些）。
    pub propagated: usize,
    /// 建立不起继承的实体，逐条给出原因。
    pub diagnostics: Vec<SpatialDiagnostic>,
}

/// 把局部变换沿父链组合成世界变换，写进 [`WorldTransform`] / [`WorldTransform3D`]。
///
/// `enabled` 为 `false`（默认）时**清掉**已有的世界变换组件并直接返回：消费者因此退回局部值，
/// 与"没有空间继承"的旧行为逐字节一致——这就是回滚档。
///
/// 遍历顺序按"父先于子"迭代求解（不是递归），因此环不会栈溢出，而是被判成诊断。
pub fn propagate_spatial(world: &mut World, enabled: bool) -> SpatialReport {
    let mut report = SpatialReport::default();
    if !enabled {
        // 关掉继承时通常**没有**世界变换组件（从没打开过）：此时直接返回，不做任何遍历。
        // 逐帧的关闭路径因此是"两次组件表查询"，而不是"遍历所有实体再删一遍"。
        if world.entities_with::<WorldTransform>().is_empty()
            && world.entities_with::<WorldTransform3D>().is_empty()
        {
            return report;
        }
        for entity in world.entities_with::<WorldTransform>() {
            let _ = world.remove::<WorldTransform>(entity);
        }
        for entity in world.entities_with::<WorldTransform3D>() {
            let _ = world.remove::<WorldTransform3D>(entity);
        }
        return report;
    }
    let entities = collect_hierarchy(world);
    propagate_entities(world, &entities, &mut report);
    report
}

/// 运行时看到的层级快照：实体 + 父 + 两个空间的局部值。
struct EntityNode {
    id: EntityId,
    parent: Option<EntityId>,
    local_2d: Option<Transform>,
    local_3d: Option<Transform3D>,
}

fn collect_hierarchy(world: &World) -> Vec<EntityNode> {
    let mut ids: std::collections::BTreeSet<EntityId> = std::collections::BTreeSet::new();
    world.for_each::<Parent>(|id, _| {
        ids.insert(id);
    });
    // 没有 `Parent` 但有变换的实体同样要纳入：3D 实体的父节点常常自己不是任何人的子节点，
    // 漏掉它就会让子实体把局部值当世界值（父的旋转/缩放全部丢失）。
    world.for_each::<Transform>(|id, _| {
        ids.insert(id);
    });
    world.for_each::<Transform3D>(|id, _| {
        ids.insert(id);
    });
    let mut nodes = Vec::with_capacity(ids.len());
    for id in ids {
        let parent = world
            .get::<Parent>(id)
            .ok()
            .flatten()
            .map(|parent| parent.0);
        let local_2d = world.get::<Transform>(id).ok().flatten().copied();
        let local_3d = world.get::<Transform3D>(id).ok().flatten().copied();
        nodes.push(EntityNode {
            id,
            parent,
            local_2d,
            local_3d,
        });
    }
    nodes
}

fn propagate_entities(world: &mut World, nodes: &[EntityNode], report: &mut SpatialReport) {
    let index_by_id = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id, index))
        .collect::<std::collections::BTreeMap<_, _>>();
    // `state`: 0 = 未处理，1 = 处理中（在栈上，遇到即为环），2 = 已完成。
    let mut state = vec![0_u8; nodes.len()];
    let mut world_2d = vec![None; nodes.len()];
    let mut world_3d = vec![None; nodes.len()];
    for start in 0..nodes.len() {
        if state[start] != 0 {
            continue;
        }
        // 显式栈：先沿父链上溯，再自顶向下组合。
        let mut chain = Vec::new();
        let mut cursor = Some(start);
        let mut cycle = false;
        while let Some(index) = cursor {
            match state[index] {
                2 => break,
                1 => {
                    cycle = true;
                    break;
                }
                _ => {}
            }
            state[index] = 1;
            chain.push(index);
            cursor = nodes[index]
                .parent
                .and_then(|parent| index_by_id.get(&parent).copied());
        }
        if cycle {
            // 环上的实体全部保留局部值，逐条报告（不猜测谁是"根"）。
            for &index in &chain {
                state[index] = 2;
                // 成环的实体同样保留局部值当世界值（不变量：开启继承 ⇒ 有局部变换就有世界变换）。
                if nodes[index].local_2d.is_some() {
                    world_2d[index] = nodes[index].local_2d;
                }
                if nodes[index].local_3d.is_some() {
                    world_3d[index] = nodes[index].local_3d;
                }
                report.diagnostics.push(SpatialDiagnostic {
                    entity: nodes[index].id,
                    reason: SpatialDiagnosticReason::Cycle,
                });
            }
            continue;
        }
        for &index in chain.iter().rev() {
            let node = &nodes[index];
            let parent_index = node
                .parent
                .and_then(|parent| index_by_id.get(&parent).copied());
            // 父还没算出世界变换（被拒绝/成环）⇒ 自己也保留局部值。
            let parent_ready = parent_index.is_none_or(|parent| {
                state[parent] == 2 && (world_2d[parent].is_some() || world_3d[parent].is_some())
            });
            if !parent_ready && parent_index.is_some() {
                state[index] = 2;
                if node.local_2d.is_some() {
                    world_2d[index] = node.local_2d;
                }
                if node.local_3d.is_some() {
                    world_3d[index] = node.local_3d;
                }
                report.diagnostics.push(SpatialDiagnostic {
                    entity: node.id,
                    reason: SpatialDiagnosticReason::Cycle,
                });
                continue;
            }
            let (parent_2d, parent_3d) =
                parent_index.map_or((None, None), |parent| (world_2d[parent], world_3d[parent]));
            let mut ok = false;
            if let Some(local) = node.local_2d {
                // 有父就组合；组合不了（剪切/镜像）或没有父就**保留局部值当世界值**并记诊断——
                // 这样"开启继承 ⇒ 每个有局部变换的实体都有世界变换"是一条可靠不变量，
                // 提取路径因此可以整条换成 `for_each2::<WorldTransform, _>`，不必在每个实体上
                // 多做一次 `get`（WP6 量到那正是热点）。
                match parent_2d {
                    Some(parent) => match compose_2d(parent, local) {
                        Ok(composed) => {
                            world_2d[index] = Some(composed);
                            ok = true;
                        }
                        Err(error) => {
                            world_2d[index] = Some(local);
                            ok = true;
                            report.diagnostics.push(SpatialDiagnostic {
                                entity: node.id,
                                reason: SpatialDiagnosticReason::NotRepresentable(error),
                            });
                        }
                    },
                    None => {
                        world_2d[index] = Some(local);
                        ok = true;
                    }
                }
            }
            if let Some(local) = node.local_3d {
                match parent_3d {
                    Some(parent) => match compose_3d(parent, local) {
                        Ok(composed) => {
                            world_3d[index] = Some(composed);
                            ok = true;
                        }
                        Err(error) => {
                            world_3d[index] = Some(local);
                            ok = true;
                            report.diagnostics.push(SpatialDiagnostic {
                                entity: node.id,
                                reason: SpatialDiagnosticReason::NotRepresentable(error),
                            });
                        }
                    },
                    None => {
                        world_3d[index] = Some(local);
                        ok = true;
                    }
                }
            }
            state[index] = 2;
            if ok {
                report.propagated += 1;
            }
        }
    }
    // 写回组件：只在真的算出世界变换时插入。
    for (index, node) in nodes.iter().enumerate() {
        match world_2d[index] {
            Some(transform) => {
                let _ = world.insert(node.id, WorldTransform(transform));
            }
            None => {
                let _ = world.remove::<WorldTransform>(node.id);
            }
        }
        match world_3d[index] {
            Some(transform) => {
                let _ = world.insert(node.id, WorldTransform3D(transform));
            }
            None => {
                let _ = world.remove::<WorldTransform3D>(node.id);
            }
        }
    }
}

/// 2D 组合：`world = parent ∘ local`，用矩阵算再分解（剪切/镜像一律拒绝）。
fn compose_2d(parent: Transform, local: Transform) -> Result<Transform, DecomposeError> {
    let parent_local = LocalTransform::from_2d(parent).map_err(|_| DecomposeError::NotAffine)?;
    let child_local = LocalTransform::from_2d(local).map_err(|_| DecomposeError::NotAffine)?;
    compose(parent_local, child_local).and_then(|composed| {
        let transform = Transform {
            position: crate::core::transform::Vec2::new(
                composed.translation.x,
                composed.translation.y,
            ),
            rotation: rotation_z(composed),
            scale: crate::core::transform::Vec2::new(composed.scale.x, composed.scale.y),
        };
        // 2D 结果必须仍在平面上（z 位移/缩放为 1、旋转只绕 Z），否则说明父链把实体推出了平面。
        if composed.translation.z.abs() > 1e-4 || (composed.scale.z - 1.0).abs() > 1e-4 {
            return Err(DecomposeError::Shear {
                amount: composed.translation.z.abs(),
            });
        }
        if !transform.position.is_finite() {
            return Err(DecomposeError::NotAffine);
        }
        Ok(transform)
    })
}

/// 3D 组合：`world = parent ∘ local`。
fn compose_3d(parent: Transform3D, local: Transform3D) -> Result<Transform3D, DecomposeError> {
    compose(
        LocalTransform::from_3d(parent),
        LocalTransform::from_3d(local),
    )
    .map(|composed| Transform3D {
        position: composed.translation,
        rotation: composed.rotation,
        scale: composed.scale,
    })
}

fn compose(
    parent: LocalTransform,
    local: LocalTransform,
) -> Result<LocalTransform, DecomposeError> {
    let parent_matrix = parent.matrix().map_err(|_| DecomposeError::NotAffine)?;
    let local_matrix = local.matrix().map_err(|_| DecomposeError::NotAffine)?;
    LocalTransform::from_matrix(parent_matrix * local_matrix)
}

/// 绕 Z 的旋转角（弧度）。2D 组合的结果必须只含绕 Z 分量，否则拒绝。
fn rotation_z(local: LocalTransform) -> f32 {
    2.0 * local.rotation.z.atan2(local.rotation.w)
}

/// 读实体的**世界**变换（2D）：有传播结果就用它，否则退回局部 `Transform`。
pub fn world_transform_2d(
    world: &World,
    entity: EntityId,
) -> Result<Option<Transform>, WorldError> {
    if let Ok(Some(world)) = world.get::<WorldTransform>(entity) {
        return Ok(Some(world.0));
    }
    Ok(world.get::<Transform>(entity)?.copied())
}

/// 读实体的**世界**变换（3D）：有传播结果就用它，否则退回局部 `Transform3D`。
pub fn world_transform_3d(
    world: &World,
    entity: EntityId,
) -> Result<Option<Transform3D>, WorldError> {
    if let Ok(Some(world)) = world.get::<WorldTransform3D>(entity) {
        return Ok(Some(world.0));
    }
    Ok(world.get::<Transform3D>(entity)?.copied())
}

/// 世界矩阵（3D）——渲染提取用。没有 3D 变换时返回 `None`。
pub fn world_matrix_3d(world: &World, entity: EntityId) -> Result<Option<Mat4>, WorldError> {
    let Some(transform) = world_transform_3d(world, entity)? else {
        return Ok(None);
    };
    Ok(transform.model_matrix().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::transform::Vec2;

    fn spawn_with(world: &mut World, parent: Option<EntityId>, transform: Transform) -> EntityId {
        let id = world.spawn();
        world.insert(id, transform).expect("insert transform");
        if let Some(parent) = parent {
            world.insert(id, Parent(parent)).expect("insert parent");
        }
        id
    }

    /// 关闭继承（默认）时不产生世界变换组件：消费者退回局部值，行为与旧版逐字节一致。
    #[test]
    fn disabled_propagation_leaves_no_world_components() {
        let mut world = World::new();
        let parent = spawn_with(
            &mut world,
            None,
            Transform::from_position(Vec2::new(10.0, 0.0)),
        );
        let child = spawn_with(
            &mut world,
            Some(parent),
            Transform::from_position(Vec2::new(3.0, 5.0)),
        );
        let report = propagate_spatial(&mut world, false);
        assert_eq!(report.propagated, 0);
        assert!(report.diagnostics.is_empty());
        assert!(world.get::<WorldTransform>(child).unwrap().is_none());
        assert_eq!(
            world_transform_2d(&world, child).unwrap(),
            Some(Transform::from_position(Vec2::new(3.0, 5.0))),
            "关掉继承时世界位置就是局部位置"
        );
    }

    /// 开启继承：子实体的世界变换 = 父 ∘ 子（含父缩放与旋转）。
    #[test]
    fn enabled_propagation_composes_parent_and_child() {
        let mut world = World::new();
        let parent = spawn_with(
            &mut world,
            None,
            Transform {
                position: Vec2::new(10.0, 5.0),
                rotation: 0.0,
                scale: Vec2::new(2.0, 2.0),
            },
        );
        // 迁移后的局部值：世界 x=3 ⇒ 局部 (3-10)/2 = -3.5。
        let child = spawn_with(
            &mut world,
            Some(parent),
            Transform::from_position(Vec2::new(-3.5, 0.0)),
        );
        let report = propagate_spatial(&mut world, true);
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert_eq!(report.propagated, 2, "父与子都写出世界变换");
        let world_child = world_transform_2d(&world, child).unwrap().unwrap();
        assert!(
            (world_child.position.x - 3.0).abs() < 1e-3
                && (world_child.position.y - 5.0).abs() < 1e-3,
            "世界位置应为 (3, 5)：{:?}",
            world_child.position
        );
        // 父缩放同样作用到子（局部 scale 1 × 父 scale 2）。
        assert!((world_child.scale.x - 2.0).abs() < 1e-3, "{world_child:?}");

        // 再关掉继承：世界变换组件被清掉，退回局部值。
        propagate_spatial(&mut world, false);
        assert!(world.get::<WorldTransform>(child).unwrap().is_none());
    }

    /// 非均匀父缩放 + 子旋转 = 剪切，TRS 表达不了 ⇒ **拒绝**并保留局部值，不近似。
    #[test]
    fn sheared_composition_is_refused_and_keeps_local_values() {
        let mut world = World::new();
        let parent = spawn_with(
            &mut world,
            None,
            Transform {
                position: Vec2::new(0.0, 0.0),
                rotation: 0.0,
                scale: Vec2::new(2.0, 1.0),
            },
        );
        let child = spawn_with(
            &mut world,
            Some(parent),
            Transform {
                position: Vec2::new(1.0, 0.0),
                rotation: std::f32::consts::FRAC_PI_4,
                scale: Vec2::new(1.0, 1.0),
            },
        );
        let report = propagate_spatial(&mut world, true);
        assert_eq!(report.diagnostics.len(), 1, "{report:?}");
        assert_eq!(
            report.diagnostics[0].entity, child,
            "含剪切的子实体要被点名"
        );
        assert!(matches!(
            report.diagnostics[0].reason,
            SpatialDiagnosticReason::NotRepresentable(_)
        ));
        // 被拒绝的实体**仍然**有世界变换，但值等于它自己的局部值（不变量：开启继承 ⇒
        // 有局部变换就有世界变换；提取路径因此整条换成 `for_each2::<WorldTransform, _>` 也不会漏画）。
        assert_eq!(
            world.get::<WorldTransform>(child).unwrap().map(|w| w.0),
            Some(Transform {
                position: Vec2::new(1.0, 0.0),
                rotation: std::f32::consts::FRAC_PI_4,
                scale: Vec2::new(1.0, 1.0),
            }),
            "含剪切的实体保留局部值当世界值"
        );
    }

    /// 成环不崩：环上实体全部保留局部值并逐条报告。
    #[test]
    fn cycles_are_reported_instead_of_hanging() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        world
            .insert(a, Transform::from_position(Vec2::new(1.0, 0.0)))
            .unwrap();
        world
            .insert(b, Transform::from_position(Vec2::new(2.0, 0.0)))
            .unwrap();
        world.insert(a, Parent(b)).unwrap();
        world.insert(b, Parent(a)).unwrap();
        let report = propagate_spatial(&mut world, true);
        assert_eq!(report.diagnostics.len(), 2, "{report:?}");
        assert!(
            report
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.reason == SpatialDiagnosticReason::Cycle)
        );
        assert_eq!(
            world_transform_2d(&world, a).unwrap(),
            Some(Transform::from_position(Vec2::new(1.0, 0.0)))
        );
    }

    /// 3D：父旋转 90° 绕 Y，子在其局部 +X 上 ⇒ 世界位置转到 -Z 方向。
    #[test]
    fn three_d_propagation_applies_parent_rotation() {
        let mut world = World::new();
        let parent = world.spawn();
        let rotation = crate::core::math3d::Quat::from_axis_angle(
            crate::core::math3d::Vec3::Y,
            std::f32::consts::FRAC_PI_2,
        )
        .expect("valid rotation");
        world
            .insert(
                parent,
                Transform3D {
                    position: crate::core::math3d::Vec3::ZERO,
                    rotation,
                    scale: crate::core::math3d::Vec3::new(1.0, 1.0, 1.0),
                },
            )
            .unwrap();
        let child = world.spawn();
        world
            .insert(
                child,
                Transform3D {
                    position: crate::core::math3d::Vec3::new(2.0, 0.0, 0.0),
                    rotation: crate::core::math3d::Quat::identity(),
                    scale: crate::core::math3d::Vec3::new(1.0, 1.0, 1.0),
                },
            )
            .unwrap();
        world.insert(child, Parent(parent)).unwrap();
        let report = propagate_spatial(&mut world, true);
        assert!(report.diagnostics.is_empty(), "{report:?}");
        let world_child = world_transform_3d(&world, child).unwrap().unwrap();
        assert!(
            world_child.position.x.abs() < 1e-3 && (world_child.position.z + 2.0).abs() < 1e-3,
            "绕 Y 转 90° 后局部 +X 应指向 -Z：{:?}",
            world_child.position
        );
    }
}
