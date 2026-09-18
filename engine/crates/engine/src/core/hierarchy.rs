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
    spatial_transform::{DecomposeError, LocalTransform, PropagationProblem},
    transform::{Transform, Transform3D},
    world::{World, WorldError},
};

/// 实体的空间父节点（运行时组件）。场景里的 `parent` 在实例化时落到这里。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Parent(pub EntityId);

/// 传播算出来的**世界变换**——doc 意义上的 `GlobalTransform`，由本模块的传播系统唯一派生
/// （docs/54 §8-②）。
///
/// 收敛前这里是**两个**组件（`WorldTransform(Transform)` / `WorldTransform3D(Transform3D)`），
/// 谁是权威取决于消费端读哪一个。现在只有一个组件，且它直接带**矩阵**：剪切/镜像的父子组合
/// 矩阵依然正确，只是 2D/3D 的 TRS 视图退化为局部值（同时记诊断，像素行为与收敛前一致）。
///
/// **只有开启空间继承时才会存在**——继承关闭时消费者退回局部 `Transform`/`Transform3D`，
/// 因此关掉开关就是"逐字节回到旧行为"的回滚档。
pub use crate::core::spatial_transform::GlobalTransform;

/// 一个没能建立空间继承的实体：保留局部值当世界值。
#[derive(Clone, Debug, PartialEq)]
pub struct SpatialDiagnostic {
    pub entity: EntityId,
    pub reason: SpatialDiagnosticReason,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SpatialDiagnosticReason {
    /// 父链成环（含自环）：环上实体与它们的后代都保留局部值。
    Cycle,
    /// 父 id 不在运行时实体表里（手改过的场景可能出现）：本实体按**链根**处理（世界 = 自己的
    /// 局部），但这条断链必须被点名——静默当成"没有父"会让整棵子树的位移看起来没有原因。
    MissingParent { parent: EntityId },
    /// 父在实体表里但父自己没有局部变换：它贡献不了变换，本实体与它的后代都保留局部值。
    /// （此前这种情况被报成 `Cycle`，让作者去找一个根本不存在的环。）
    ParentWithoutLocal { parent: EntityId },
    /// 祖先链上有解不了的实体（环 / 父无局部变换）：本实体保留局部值。
    UnresolvableAncestor { ancestor: EntityId },
    /// 局部或组合结果无法无损表达为 TRS（剪切/镜像/退化/奇异）。
    NotRepresentable(DecomposeError),
    /// 同一实体同时带了 2D 与 3D 局部变换（`Transform` + `Transform3D`）＝**两份局部真源**。
    ///
    /// docs/54 §8-② 要求"运行时只有一份权威 LocalTransform"；这种数据没有唯一权威，因此明确
    /// 报告而不是静默挑一个（矩阵按 3D 视图取，因为 3D 是 2D 的超集空间）。
    AmbiguousLocalTransform,
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
        // 逐帧的关闭路径因此是"一次组件表查询"，而不是"遍历所有实体再删一遍"。
        if world.entities_with::<GlobalTransform>().is_empty() {
            return report;
        }
        for entity in world.entities_with::<GlobalTransform>() {
            let _ = world.remove::<GlobalTransform>(entity);
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
    // docs/54 §8-②：父链上溯、成环判定与"什么算坏数据"的分类走
    // `spatial_transform::parent_first_plan`——**与加载期求解（`propagate_all`）同一份驱动**，
    // 因此运行时（容错）与加载期（严格）不会对同一份数据给出两套说法，差别只剩"遇到问题怎么办"。
    let plan = crate::core::spatial_transform::parent_first_plan(
        nodes.iter().map(|node| node.id),
        |id| index_by_id.get(&id).and_then(|index| nodes[*index].parent),
        |id| {
            index_by_id.get(&id).is_some_and(|index| {
                nodes[*index].local_2d.is_some() || nodes[*index].local_3d.is_some()
            })
        },
    );
    // 坏数据逐条翻译成运行时诊断（顺序确定，每个实体一条）。
    let mut dangling_parents = std::collections::BTreeSet::new();
    for problem in &plan.problems {
        let diagnostic = match problem {
            PropagationProblem::MissingParent { entity, parent } => {
                // 断链的实体**仍然求解**（按链根，见下），只是这条链必须被点名。
                dangling_parents.insert(*entity);
                SpatialDiagnostic {
                    entity: *entity,
                    reason: SpatialDiagnosticReason::MissingParent { parent: *parent },
                }
            }
            PropagationProblem::ParentWithoutLocal { entity, parent } => SpatialDiagnostic {
                entity: *entity,
                reason: SpatialDiagnosticReason::ParentWithoutLocal { parent: *parent },
            },
            PropagationProblem::Cycle(members) => {
                // 环上每个成员各一条（不猜测谁是"根"）。
                for member in members {
                    report.diagnostics.push(SpatialDiagnostic {
                        entity: *member,
                        reason: SpatialDiagnosticReason::Cycle,
                    });
                }
                continue;
            }
            PropagationProblem::UnresolvableAncestor { entity, ancestor } => SpatialDiagnostic {
                entity: *entity,
                reason: SpatialDiagnosticReason::UnresolvableAncestor {
                    ancestor: *ancestor,
                },
            },
        };
        report.diagnostics.push(diagnostic);
    }
    // `state[index] == 2` = 该实体已按父先序求解（其余实体保留局部值当世界值）。
    let mut state = vec![0_u8; nodes.len()];
    // 每个空间的**世界矩阵**：`Global(child) = Global(parent) * Local(child)`。矩阵是派生真源，
    // 剪切/镜像时它依然正确，只是压不成 TRS 视图。
    let mut matrix_2d: Vec<Option<Mat4>> = vec![None; nodes.len()];
    let mut matrix_3d: Vec<Option<Mat4>> = vec![None; nodes.len()];
    // 每个空间的 TRS **视图**：压不平时退回局部值（像素行为与收敛前一致）并记诊断。
    let mut world_2d = vec![None; nodes.len()];
    let mut world_3d = vec![None; nodes.len()];
    for &id in &plan.ordered {
        let index = index_by_id[&id];
        {
            let node = &nodes[index];
            // 断链的父当作"没有父"（链根语义）；其余情况父一定排在前面（`ordered` 的性质）。
            let parent_index = if dangling_parents.contains(&id) {
                None
            } else {
                node.parent
                    .and_then(|parent| index_by_id.get(&parent).copied())
            };
            // 父还没算出世界变换（被拒绝/成环）⇒ 自己也保留局部值。
            let parent_ready = parent_index.is_none_or(|parent| {
                state[parent] == 2 && (world_2d[parent].is_some() || world_3d[parent].is_some())
            });
            if !parent_ready && parent_index.is_some() {
                state[index] = 2;
                if node.local_2d.is_some() {
                    world_2d[index] = node.local_2d;
                    matrix_2d[index] = node.local_2d.and_then(|local| local_matrix_2d(local).ok());
                }
                if node.local_3d.is_some() {
                    world_3d[index] = node.local_3d;
                    matrix_3d[index] = node.local_3d.and_then(|local| local.model_matrix().ok());
                }
                report.diagnostics.push(SpatialDiagnostic {
                    entity: node.id,
                    reason: SpatialDiagnosticReason::UnresolvableAncestor {
                        ancestor: parent_index.map_or(node.id, |parent| nodes[parent].id),
                    },
                });
                continue;
            }
            let (parent_2d, parent_3d) =
                parent_index.map_or((None, None), |parent| (world_2d[parent], world_3d[parent]));
            let (parent_matrix_2d, parent_matrix_3d) = parent_index
                .map_or((None, None), |parent| {
                    (matrix_2d[parent], matrix_3d[parent])
                });
            let mut ok = false;
            if let Some(local) = node.local_2d {
                // 矩阵先算准：`Global(child) = Global(parent) * Local(child)`，剪切/镜像时
                // 矩阵依然正确（只是压不成 TRS）。
                if let Ok(local_matrix) = local_matrix_2d(local) {
                    matrix_2d[index] = Some(match parent_matrix_2d {
                        Some(parent) => parent * local_matrix,
                        None => local_matrix,
                    });
                }
                // 有父就组合；组合不了（剪切/镜像）或没有父就**保留局部值当世界值**并记诊断——
                // 这样"开启继承 ⇒ 每个有局部变换的实体都有世界变换"是一条可靠不变量，
                // 提取路径因此可以整条换成 `for_each2::<GlobalTransform, _>`，不必在每个实体上
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
                if let Ok(local_matrix) = local.model_matrix() {
                    matrix_3d[index] = Some(match parent_matrix_3d {
                        Some(parent) => parent * local_matrix,
                        None => local_matrix,
                    });
                }
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
            // docs/54 §8-②：一个实体同时有 2D 与 3D 局部变换 = **两份局部真源**。
            // 不静默挑一个当权威：明确报告，让作者去删掉多余的那个（世界矩阵此时按 3D 视图取，
            // 因为 3D 是 2D 的超集空间）。
            if node.local_2d.is_some() && node.local_3d.is_some() {
                report.diagnostics.push(SpatialDiagnostic {
                    entity: node.id,
                    reason: SpatialDiagnosticReason::AmbiguousLocalTransform,
                });
            }
            state[index] = 2;
            if ok {
                report.propagated += 1;
            }
        }
    }
    // 解不了的实体（环成员、以及祖先链上有坏数据的后代）**保留局部值当世界值**：这样
    // "开启继承 ⇒ 每个有局部变换的实体都有世界变换"仍是一条可靠不变量，提取路径不会漏画。
    for (index, node) in nodes.iter().enumerate() {
        if state[index] == 2 {
            continue;
        }
        if node.local_2d.is_some() {
            world_2d[index] = node.local_2d;
            matrix_2d[index] = node.local_2d.and_then(|local| local_matrix_2d(local).ok());
        }
        if node.local_3d.is_some() {
            world_3d[index] = node.local_3d;
            matrix_3d[index] = node.local_3d.and_then(|local| local.model_matrix().ok());
        }
    }
    // 写回**唯一**的派生组件 `GlobalTransform`：矩阵是真相，2D/3D 视图由同一次传播给出。
    // 没有局部变换的实体不写（也不残留上一帧的值）。
    for (index, node) in nodes.iter().enumerate() {
        let matrix = matrix_3d[index].or(matrix_2d[index]);
        match matrix {
            Some(matrix) => {
                let _ = world.insert(
                    node.id,
                    GlobalTransform::with_views(matrix, world_2d[index], world_3d[index]),
                );
            }
            None => {
                let _ = world.remove::<GlobalTransform>(node.id);
            }
        }
    }
}

/// 2D 局部变换的矩阵。非法（非有限/退化）时返回错误——此时矩阵不写，视图按既有规则退回局部值。
fn local_matrix_2d(local: Transform) -> Result<Mat4, DecomposeError> {
    LocalTransform::from_2d(local)
        .map_err(|_| DecomposeError::NotAffine)?
        .matrix()
        .map_err(|_| DecomposeError::NotAffine)
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

/// 读实体的**世界**变换（2D）：有传播结果就用它的平面视图，否则退回局部 `Transform`。
pub fn world_transform_2d(
    world: &World,
    entity: EntityId,
) -> Result<Option<Transform>, WorldError> {
    if let Ok(Some(global)) = world.get::<GlobalTransform>(entity)
        && let Some(plane) = global.plane()
    {
        return Ok(Some(plane));
    }
    Ok(world.get::<Transform>(entity)?.copied())
}

/// 读实体的**世界**变换（3D）：有传播结果就用它的空间视图，否则退回局部 `Transform3D`。
pub fn world_transform_3d(
    world: &World,
    entity: EntityId,
) -> Result<Option<Transform3D>, WorldError> {
    if let Ok(Some(global)) = world.get::<GlobalTransform>(entity)
        && let Some(space) = global.space()
    {
        return Ok(Some(space));
    }
    Ok(world.get::<Transform3D>(entity)?.copied())
}

/// 读实体的**世界**变换（3D），**不要求本帧已经传播过**。
///
/// 有 `GlobalTransform` 就直接用（快路径，与提取路径同一份数据）；没有就沿 `Parent` 链现算一次
/// ——相机姿态这类一帧只问一个实体的消费者，因此不必依赖"调用者先跑过传播"这个隐含顺序
/// （docs/54 §8-⑦：同一空间语义不该由调用顺序决定）。成环或缺父时退回局部值。
pub fn world_transform_3d_lazy(
    world: &World,
    entity: EntityId,
) -> Result<Option<Transform3D>, WorldError> {
    if let Ok(Some(global)) = world.get::<GlobalTransform>(entity)
        && let Some(space) = global.space()
    {
        return Ok(Some(space));
    }
    let Some(local) = world.get::<Transform3D>(entity)?.copied() else {
        return Ok(None);
    };
    let Ok(local_matrix) = local.model_matrix() else {
        return Ok(Some(local));
    };
    let mut matrix = local_matrix;
    let mut seen = std::collections::BTreeSet::new();
    seen.insert(entity);
    let mut cursor = world
        .get::<Parent>(entity)
        .ok()
        .flatten()
        .map(|parent| parent.0);
    while let Some(parent) = cursor {
        if !seen.insert(parent) {
            break; // 成环：停止上溯，用已经组合出来的那一份
        }
        let Some(parent_local) = world.get::<Transform3D>(parent).ok().flatten().copied() else {
            break;
        };
        let Ok(parent_matrix) = parent_local.model_matrix() else {
            break;
        };
        matrix = parent_matrix * matrix;
        cursor = world
            .get::<Parent>(parent)
            .ok()
            .flatten()
            .map(|parent| parent.0);
    }
    Ok(LocalTransform::from_matrix(matrix)
        .ok()
        .map(|composed| Transform3D {
            position: composed.translation,
            rotation: composed.rotation,
            scale: composed.scale,
        }))
}

/// 读实体的**世界矩阵**（派生真源本身）——渲染提取用。
///
/// 收敛前这里是从分解出来的 `Transform3D` 再算一遍模型矩阵；现在直接返回传播系统算出的
/// `Global(parent) * Local(child)`，因此**剪切/镜像的父子组合也能拿到正确的世界矩阵**
/// （旧路径此时只能退回局部值）。没有 3D 局部变换时返回 `None`。
pub fn world_matrix_3d(world: &World, entity: EntityId) -> Result<Option<Mat4>, WorldError> {
    if let Ok(Some(global)) = world.get::<GlobalTransform>(entity)
        && global.space().is_some()
    {
        return Ok(Some(global.matrix()));
    }
    let Some(transform) = world.get::<Transform3D>(entity)?.copied() else {
        return Ok(None);
    };
    Ok(transform.model_matrix().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::math3d::{Quat, Vec3};
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
        assert!(world.get::<GlobalTransform>(child).unwrap().is_none());
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
        assert!(world.get::<GlobalTransform>(child).unwrap().is_none());
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
        // 有局部变换就有世界变换；提取路径因此整条换成 `for_each2::<GlobalTransform, _>` 也不会漏画）。
        assert_eq!(
            world
                .get::<GlobalTransform>(child)
                .unwrap()
                .and_then(|g| g.plane()),
            Some(Transform {
                position: Vec2::new(1.0, 0.0),
                rotation: std::f32::consts::FRAC_PI_4,
                scale: Vec2::new(1.0, 1.0),
            }),
            "含剪切的实体保留局部值当世界值"
        );
        // docs/54 §8-②：**矩阵不受影响**——`Global(child) = Global(parent) * Local(child)` 在
        // 剪切下依然精确。派生真源因此比"退化后的 TRS 视图"更强：3D 提取直接用它。
        let global = world
            .get::<GlobalTransform>(child)
            .unwrap()
            .expect("child has a global transform");
        let expected = {
            let parent = LocalTransform::from_2d(Transform {
                position: Vec2::zero(),
                rotation: 0.0,
                scale: Vec2::new(2.0, 1.0),
            })
            .unwrap()
            .matrix()
            .unwrap();
            let local = LocalTransform::from_2d(Transform {
                position: Vec2::new(1.0, 0.0),
                rotation: std::f32::consts::FRAC_PI_4,
                scale: Vec2::new(1.0, 1.0),
            })
            .unwrap()
            .matrix()
            .unwrap();
            parent * local
        };
        assert_eq!(
            global.matrix(),
            expected,
            "剪切组合下的世界矩阵必须仍是 parent ∘ local（而不是退回局部值）"
        );
    }

    /// docs/54 §8-②：同一实体同时带 `Transform` 与 `Transform3D` ＝两份局部真源，必须**报告**
    /// 而不是静默挑一个；世界矩阵此时按 3D 视图（超集空间）取。
    #[test]
    fn ambiguous_local_transforms_are_reported_and_the_3d_matrix_wins() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Transform::from_position(Vec2::new(3.0, 0.0)))
            .unwrap();
        world
            .insert(
                entity,
                Transform3D {
                    position: Vec3::new(0.0, 7.0, 0.0),
                    rotation: Quat::identity(),
                    scale: Vec3::new(1.0, 1.0, 1.0),
                },
            )
            .unwrap();
        // 有 `Parent` 才会走传播（与运行时的 `spatial_active` 判据一致）。
        let parent = world.spawn();
        world
            .insert(parent, Transform::from_position(Vec2::zero()))
            .unwrap();
        world.insert(entity, Parent(parent)).unwrap();

        let report = propagate_spatial(&mut world, true);
        assert!(
            report.diagnostics.iter().any(|d| d.entity == entity
                && d.reason == SpatialDiagnosticReason::AmbiguousLocalTransform),
            "同存两种局部变换必须被点名：{report:?}"
        );
        let global = world
            .get::<GlobalTransform>(entity)
            .unwrap()
            .expect("global transform");
        assert_eq!(
            global.matrix().columns[3][1],
            7.0,
            "矩阵按 3D 视图取（3D 是 2D 的超集空间）"
        );
        // 两个视图都还在：2D 视图来自 2D 链、3D 视图来自 3D 链。
        assert_eq!(global.plane().expect("plane view").position.x, 3.0);
        assert_eq!(global.space().expect("space view").position.y, 7.0);
    }

    /// docs/54 §8-②：成环不崩，且**后代**得到的是"祖先不可解"而不是又一条 `Cycle`
    /// ——把后代报成环会让作者去找一个根本不存在的环。
    #[test]
    fn cycle_descendants_are_reported_as_unresolvable_ancestors() {
        let mut world = World::new();
        let a = spawn_with(
            &mut world,
            None,
            Transform::from_position(Vec2::new(1.0, 0.0)),
        );
        let b = spawn_with(
            &mut world,
            Some(a),
            Transform::from_position(Vec2::new(2.0, 0.0)),
        );
        // 让 a 指向 b：a ↔ b 成环；c 是 a 的子，d 是 c 的子。
        world.insert(a, Parent(b)).unwrap();
        let c = spawn_with(
            &mut world,
            Some(a),
            Transform::from_position(Vec2::new(3.0, 0.0)),
        );
        let d = spawn_with(
            &mut world,
            Some(c),
            Transform::from_position(Vec2::new(4.0, 0.0)),
        );
        let report = propagate_spatial(&mut world, true);
        let reason = |entity: EntityId| {
            report
                .diagnostics
                .iter()
                .find(|diagnostic| diagnostic.entity == entity)
                .map(|diagnostic| diagnostic.reason.clone())
        };
        assert_eq!(reason(a), Some(SpatialDiagnosticReason::Cycle));
        assert_eq!(reason(b), Some(SpatialDiagnosticReason::Cycle));
        assert_eq!(
            reason(c),
            Some(SpatialDiagnosticReason::UnresolvableAncestor { ancestor: a }),
            "后代必须报「祖先不可解」：{report:?}"
        );
        assert_eq!(
            reason(d),
            Some(SpatialDiagnosticReason::UnresolvableAncestor { ancestor: c }),
            "{report:?}"
        );
        // 不变量仍然成立：有局部变换就有世界变换（值为各自的局部值）。
        for (entity, position) in [(a, 1.0), (b, 2.0), (c, 3.0), (d, 4.0)] {
            assert_eq!(
                world_transform_2d(&world, entity).unwrap(),
                Some(Transform::from_position(Vec2::new(position, 0.0))),
                "实体 {entity:?} 必须保留局部值当世界值"
            );
        }
    }

    /// docs/54 §8-②：断链的 `Parent`（指向不存在的实体）此前被**静默**当成"没有父"——子树因此
    /// 悄悄少一层位移而毫无线索。现在仍按链根求解（世界 = 自己的局部），但必须点名。
    #[test]
    fn dangling_parents_are_reported_while_still_solving_as_a_chain_root() {
        let mut world = World::new();
        let ghost = EntityId::from_raw(999, 1);
        let child = spawn_with(
            &mut world,
            Some(ghost),
            Transform::from_position(Vec2::new(3.0, 0.0)),
        );
        let report = propagate_spatial(&mut world, true);
        assert_eq!(
            report.diagnostics,
            vec![SpatialDiagnostic {
                entity: child,
                reason: SpatialDiagnosticReason::MissingParent { parent: ghost },
            }],
            "断链必须被点名"
        );
        assert_eq!(
            world_transform_2d(&world, child).unwrap(),
            Some(Transform::from_position(Vec2::new(3.0, 0.0)))
        );
        assert_eq!(report.propagated, 1, "断链实体仍按链根求解");
    }

    /// docs/54 §8-②：父在实体表里但父自己没有局部变换：本实体与后代保留局部值，
    /// 原因必须是 `ParentWithoutLocal` / `UnresolvableAncestor`（此前一律报成 `Cycle`）。
    #[test]
    fn a_transformless_parent_is_reported_with_its_real_reason() {
        let mut world = World::new();
        let root = spawn_with(
            &mut world,
            None,
            Transform::from_position(Vec2::new(10.0, 0.0)),
        );
        // `middle` 有父但没有局部变换：它贡献不了任何变换。
        let middle = world.spawn();
        world.insert(middle, Parent(root)).unwrap();
        let child = spawn_with(
            &mut world,
            Some(middle),
            Transform::from_position(Vec2::new(1.0, 0.0)),
        );
        let report = propagate_spatial(&mut world, true);
        assert_eq!(
            report.diagnostics,
            vec![SpatialDiagnostic {
                entity: child,
                reason: SpatialDiagnosticReason::ParentWithoutLocal { parent: middle },
            }],
            "{report:?}"
        );
        assert_eq!(
            world_transform_2d(&world, child).unwrap(),
            Some(Transform::from_position(Vec2::new(1.0, 0.0))),
            "父没有变换 ⇒ 子既不能凭空组合，也不能被当成「父在原点」"
        );
        assert!(
            world.get::<GlobalTransform>(middle).unwrap().is_none(),
            "没有局部变换的实体不该有派生的世界变换"
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

    /// docs/54 §8-②：世界侧的传播（`hierarchy`，容错、逐帧、面向 `World`）与加载侧的求解
    /// （`spatial_transform::propagate_all`，严格、面向场景字典）**必须给出同一份世界矩阵**。
    ///
    /// 两者是同一套语义的两个入口（一个容忍坏数据并记诊断，一个直接报错），因此"它们算得一样"
    /// 是要被测试钉住的不变量：任何一侧改了组合规则，这条立刻判红。
    #[test]
    fn runtime_propagation_agrees_with_the_scene_side_solver() {
        use crate::core::scene::SceneEntityId;
        use crate::core::spatial_transform::{LocalTransform as MathLocal, propagate_all};
        use std::collections::BTreeMap;

        let mut world = World::new();
        // 深度 3 的 3D 链：父旋转 + **等比**缩放（非均匀缩放 × 子旋转会产生剪切，
        // 那是另一条测试覆盖的拒绝路径），子再平移。
        let root = world.spawn();
        let mid = world.spawn();
        let leaf = world.spawn();
        let chain = [
            (
                root,
                Transform3D {
                    position: Vec3::new(1.0, 0.0, 0.0),
                    rotation: Quat::from_axis_angle(Vec3::Y, 0.5).unwrap(),
                    scale: Vec3::new(2.0, 2.0, 2.0),
                },
            ),
            (
                mid,
                Transform3D {
                    position: Vec3::new(0.0, 1.0, 0.0),
                    rotation: Quat::from_axis_angle(Vec3::X, 0.25).unwrap(),
                    scale: Vec3::new(1.0, 1.0, 1.0),
                },
            ),
            (
                leaf,
                Transform3D {
                    position: Vec3::new(0.0, 0.0, 2.0),
                    rotation: Quat::identity(),
                    scale: Vec3::new(1.0, 1.0, 1.0),
                },
            ),
        ];
        for (entity, transform) in chain {
            world.insert(entity, transform).unwrap();
        }
        world.insert(mid, Parent(root)).unwrap();
        world.insert(leaf, Parent(mid)).unwrap();

        let report = propagate_spatial(&mut world, true);
        assert!(report.diagnostics.is_empty(), "{report:?}");

        let mut locals: BTreeMap<SceneEntityId, MathLocal> = BTreeMap::new();
        let mut parents: BTreeMap<SceneEntityId, SceneEntityId> = BTreeMap::new();
        for (index, (_entity, transform)) in chain.iter().enumerate() {
            locals.insert(
                SceneEntityId(index as u64 + 1),
                MathLocal::from_3d(*transform),
            );
        }
        parents.insert(SceneEntityId(2), SceneEntityId(1));
        parents.insert(SceneEntityId(3), SceneEntityId(2));
        let solved = propagate_all(&locals, &parents).expect("scene-side solve");

        for (index, (entity, _)) in chain.iter().enumerate() {
            let scene_id = SceneEntityId(index as u64 + 1);
            let runtime_matrix = world
                .get::<GlobalTransform>(*entity)
                .unwrap()
                .expect("global transform")
                .matrix();
            let scene_matrix = solved[&scene_id].matrix();
            for column in 0..4 {
                for row in 0..4 {
                    assert!(
                        (runtime_matrix.columns[column][row] - scene_matrix.columns[column][row])
                            .abs()
                            < 1e-5,
                        "第 {index} 层的世界矩阵两侧不一致：运行时 {runtime_matrix:?} vs 场景侧 {scene_matrix:?}"
                    );
                }
            }
        }
    }
}
