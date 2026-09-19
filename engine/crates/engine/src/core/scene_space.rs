//! **文档（编辑态）的空间语义**：让编辑器、迁移工具与加载期校验和运行时共用同一套父子空间规则
//! （docs/54 §8-⑦）。
//!
//! 运行时从 ECS 传播世界变换，而文档侧手里只有 [`Scene`]，没有 ECS。两边各算一套就会出现
//! "手柄在人不在的地方"（标记用局部值、画面用世界值）与"拖完跳一下"（把世界值写进局部槽）。
//!
//! 与 [`crate::core::hierarchy`] 的关系：**组合规则只有一份**——这里用
//! [`crate::core::spatial_transform::propagate_all`] 求解 `Global(parent) * Local(child)`，
//! 与运行时逐帧传播的 `hierarchy::propagate_spatial` 由等价性测试钉住必须一致。

use std::collections::BTreeMap;

use crate::core::{
    math3d::Mat4,
    scene::{Scene, SceneEntityId},
    spatial_transform::{GlobalTransform, LocalTransform},
    transform::{Transform, Transform3D},
};

/// 场景元数据里声明空间语义的键（迁移工具写入的就是它，见 `scene_migration`）。
pub const SPATIAL_META_KEY: &str = "spatial";

/// 场景声明的 `parent` 语义（docs/54 §4.1）。
///
/// §4.1 的原文是"旧 v2 `parent` 默认仍是组织关系，不自动解释为局部空间"。此前运行时**没有**读
/// 这个声明：`RuntimeScene` 的继承开关默认是开的，于是任何一个带 `parent` 边的 v2 场景都会被
/// 自动按"父子组合"解释——正是这条要求要挡住的静默语义变化。现在默认值来自场景自身：
///
/// - 未声明 ⇒ [`Self::Organizational`]（v2 的历史语义：`parent` 只表达层级/分组）；
/// - `spatial=organizational` ⇒ 显式声明同一件事；
/// - `spatial=local` ⇒ [`Self::Inherit`]（迁移后的场景：局部值就是局部空间的值，父子参与组合）。
///
/// 宿主仍可用 `RuntimeScene::set_spatial_inheritance` 覆盖（回滚档），但**默认不再由宿主猜**。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpatialMode {
    /// `parent` 只是组织关系：子实体的局部变换**不**参与父子组合。
    Organizational,
    /// `parent` 是空间关系：`Global(child) = Global(parent) * Local(child)`。
    Inherit,
}

impl SpatialMode {
    /// 元数据里的取值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Organizational => "organizational",
            Self::Inherit => "local",
        }
    }

    /// 是否要建立父子空间继承。
    #[must_use]
    pub const fn inherits(self) -> bool {
        matches!(self, Self::Inherit)
    }

    /// 解析元数据取值。`None` = 未声明（走 v2 默认）。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "local" | "inherit" => Some(Self::Inherit),
            "organizational" | "organize" => Some(Self::Organizational),
            _ => None,
        }
    }
}

/// 读场景声明的 `parent` 语义。未声明或值非法都按 v2 历史默认（组织关系）处理；非法值另外由
/// [`spatial_mode_issue`] 点名，不是静默吞掉。
#[must_use]
pub fn spatial_mode(scene: &Scene) -> SpatialMode {
    scene
        .metadata
        .get(SPATIAL_META_KEY)
        .and_then(|value| SpatialMode::parse(value))
        .unwrap_or(SpatialMode::Organizational)
}

/// 场景的空间声明是否有问题（未声明是正常的，非法值不是）。
#[must_use]
pub fn spatial_mode_issue(scene: &Scene) -> Option<String> {
    let value = scene.metadata.get(SPATIAL_META_KEY)?;
    if SpatialMode::parse(value).is_some() {
        return None;
    }
    Some(format!(
        "{SPATIAL_META_KEY}={value} 不是已知取值（应为 local 或 organizational），已按 v2 默认（组织关系）处理"
    ))
}

/// 文档里每个实体的**局部**变换（2D 的 `Transform` 升维成平面 TRS，3D 的 `Transform3D` 直取）。
///
/// 同时带 2D 与 3D 局部变换的实体取 3D（超集空间）——与运行时"两份真源会被点名、矩阵按 3D 取"
/// 的规则一致（docs/54 §8-②）。
#[must_use]
pub fn local_transforms(scene: &Scene) -> BTreeMap<SceneEntityId, LocalTransform> {
    let mut locals = BTreeMap::new();
    for entity in scene.entities.values() {
        if let Some(raw) = entity.components.get("Transform3D")
            && let Ok(transform) = Transform3D::from_component_str(raw)
        {
            locals.insert(entity.id, LocalTransform::from_3d(transform));
            continue;
        }
        if let Some(raw) = entity.components.get("Transform")
            && let Ok(transform) = Transform::from_component_str(raw)
            && let Ok(local) = LocalTransform::from_2d(transform)
        {
            locals.insert(entity.id, local);
        }
    }
    locals
}

/// 文档里的 parent 指针（只保留指向**同一文档内实体**的边；自环与悬空父按"没有父"处理）。
#[must_use]
pub fn parents(scene: &Scene) -> BTreeMap<SceneEntityId, SceneEntityId> {
    let mut parents = BTreeMap::new();
    for entity in scene.entities.values() {
        if let Some(parent) = entity.parent
            && scene.entities.contains_key(&parent)
            && parent != entity.id
        {
            parents.insert(entity.id, parent);
        }
    }
    parents
}

/// 文档里 parent 指针的**诊断**（docs/54 §8-②）：哪些实体的父指向不存在的实体或自己。
///
/// [`parents`] 会把这类边当作"没有父"处理（容错，与运行时的断链语义一致），但**必须可发现**：
/// 一份手改坏了的场景不该表现为"子树莫名其妙少了一层位移"。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParentLinkIssue {
    /// 父 id 不在同一文档里。
    Dangling {
        entity: SceneEntityId,
        parent: SceneEntityId,
    },
    /// 父指向自己。
    SelfParent(SceneEntityId),
}

/// 逐条列出文档里的坏 parent 边（顺序确定）。
#[must_use]
pub fn parent_link_issues(scene: &Scene) -> Vec<ParentLinkIssue> {
    let mut issues = Vec::new();
    for entity in scene.entities.values() {
        let Some(parent) = entity.parent else {
            continue;
        };
        if parent == entity.id {
            issues.push(ParentLinkIssue::SelfParent(entity.id));
        } else if !scene.entities.contains_key(&parent) {
            issues.push(ParentLinkIssue::Dangling {
                entity: entity.id,
                parent,
            });
        }
    }
    issues
}

/// 一条"一个实体有几份局部变换真源"的审计结论（docs/54 §8-②）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalTransformTruth {
    /// 只有 2D 局部变换。
    Plane2d,
    /// 只有 3D 局部变换（或 2D 组件的载荷非法——非法载荷由 typed 编译单独诊断）。
    Space3d,
    /// 两者都有：**两份局部真源**，没有唯一权威。运行时按 3D 取矩阵并报
    /// `AmbiguousLocalTransform` 诊断，但正确的做法是作者删掉多余的那一个。
    Ambiguous,
}

/// 实体携带哪一份局部变换。
#[must_use]
pub fn local_transform_truth(scene: &Scene, entity: SceneEntityId) -> Option<LocalTransformTruth> {
    let components = &scene.entities.get(&entity)?.components;
    match (
        components.contains_key("Transform"),
        components.contains_key("Transform3D"),
    ) {
        (true, true) => Some(LocalTransformTruth::Ambiguous),
        (false, true) => Some(LocalTransformTruth::Space3d),
        (true, false) => Some(LocalTransformTruth::Plane2d),
        (false, false) => None,
    }
}

/// 审计整份文档的局部真源：返回所有**同时带 2D 与 3D 局部变换**的实体（顺序确定）。
///
/// 这是 docs/54 §8-② "运行时只有一份权威 LocalTransform" 的**文档侧**可执行判据：运行时与文档侧
/// 各自都会在使用时报告/取 3D，但"作者写了两个"这件事必须在批量检查里能被点出来——仓库内的示例
/// 场景由 `scene_migration` 的仓库级测试逐份断言为空集。
#[must_use]
pub fn ambiguous_local_transform_entities(scene: &Scene) -> Vec<SceneEntityId> {
    scene
        .entities
        .values()
        .filter(|entity| {
            local_transform_truth(scene, entity.id) == Some(LocalTransformTruth::Ambiguous)
        })
        .map(|entity| entity.id)
        .collect()
}

/// 文档里每个实体的**世界矩阵**——与运行时同一套规则：父先序、`Global = Global(parent) * Local`。
///
/// 求解不出来的实体（成环、父不可用、局部非法）不出现在结果里，调用方应退回局部值。
///
/// docs/54 §4.1：场景声明为**组织关系**（v2 默认）时 `parent` 不参与空间组合，世界矩阵就是各实体
/// 自己的局部矩阵——文档侧与运行时侧因此由**同一份声明**决定，不会一个组合一个不组合（那正是
/// "手柄在人不在的地方"的来源）。
#[must_use]
pub fn world_matrices(scene: &Scene) -> BTreeMap<SceneEntityId, Mat4> {
    let locals = local_transforms(scene);
    if spatial_mode(scene) == SpatialMode::Organizational {
        return locals
            .iter()
            .filter_map(|(entity, local)| local.matrix().ok().map(|matrix| (*entity, matrix)))
            .collect();
    }
    let parents = parents(scene);
    match crate::core::spatial_transform::propagate_all(&locals, &parents) {
        Ok(globals) => globals
            .into_iter()
            .map(|(entity, global)| (entity, global.matrix()))
            .collect(),
        // 成环等硬错误：能解多少解多少（按"无父"逐个解），不因为一个坏实体让整份文档失去世界变换。
        Err(_) => locals
            .iter()
            .filter_map(|(entity, local)| local.matrix().ok().map(|matrix| (*entity, matrix)))
            .collect(),
    }
}

/// 实体的世界变换（3D 视图）：优先矩阵分解，失败退回局部 `Transform3D`。
#[must_use]
pub fn world_transform_3d(scene: &Scene, entity: SceneEntityId) -> Option<Transform3D> {
    if let Some(matrix) = world_matrices(scene).get(&entity)
        && let Ok(local) = LocalTransform::from_matrix(*matrix)
    {
        return Some(Transform3D {
            position: local.translation,
            rotation: local.rotation,
            scale: local.scale,
        });
    }
    scene
        .entities
        .get(&entity)?
        .components
        .get("Transform3D")
        .and_then(|raw| Transform3D::from_component_str(raw).ok())
}

/// 实体的世界变换（2D 平面视图）。
///
/// 矩阵压不成平面 TRS（含 z 位移/缩放或绕 X-Y 旋转）时返回 `None`：2D 侧**不接受**把一个
/// 空间变换硬压平（docs/54 §1.2），调用方应把该实体当成"不在 2D 平面上"。
#[must_use]
pub fn world_transform_2d(scene: &Scene, entity: SceneEntityId) -> Option<Transform> {
    let matrix = world_matrices(scene).get(&entity).copied()?;
    let local = LocalTransform::from_matrix(matrix).ok()?;
    crate::core::spatial_transform::plane_local(local)
}

/// 实体父节点的世界矩阵（没有父/父不可解 ⇒ `None`）。
///
/// 组织关系（v2 默认）下恒为 `None`：父不参与空间组合，因此"在父之下保持世界矩阵"在这里就是
/// "保持自己的局部值"。调用方（Gizmo 提交、换父 keep-world）因此不需要自己判断场景模式。
#[must_use]
pub fn parent_world_matrix(scene: &Scene, entity: SceneEntityId) -> Option<Mat4> {
    if spatial_mode(scene) == SpatialMode::Organizational {
        return None;
    }
    let parent = scene.entities.get(&entity)?.parent?;
    world_matrices(scene).get(&parent).copied()
}

/// 求"在给定父之下仍保持世界矩阵 `world`"的局部变换——Gizmo 提交与换父 keep-world 共用。
///
/// 无法无损表达（剪切/镜像/退化）时返回 `Err`：调用方应拒绝写回而不是静默近似（docs/54 §1.2）。
pub fn local_keeping_world(
    parent_world: Option<Mat4>,
    world: Mat4,
) -> Result<LocalTransform, String> {
    GlobalTransform::from_matrix(world)
        .local_relative_to(parent_world.map(GlobalTransform::from_matrix))
        .map_err(|error| error.to_string())
}

/// 把一批**目标世界变换**换算成父先序的**新局部变换**（不修改文档）。
///
/// 与"逐个实体用拖动前的父世界反解"的区别在**父与子同批移动**时：这里先按父先序求每个实体的
/// 新世界矩阵，再用**新**父世界解子实体的局部值，因此整棵子树一起拖不会出现子实体被父的位移
/// 二次叠加。返回的是可安全写回文档的局部变换（解不出来的实体不在结果里，调用方应跳过并提示）。
#[must_use]
pub fn locals_for_world_targets(
    scene: &Scene,
    targets: &BTreeMap<SceneEntityId, LocalTransform>,
) -> BTreeMap<SceneEntityId, LocalTransform> {
    let locals = local_transforms(scene);
    let parents = parents(scene);
    // 父先序顺序：按"沿父链上溯的长度"排序（深度小者在前）。环上的实体排在最后并被跳过。
    let mut order: Vec<SceneEntityId> = scene.entities.keys().copied().collect();
    order.sort_by_key(|entity| {
        let mut depth = 0_u32;
        let mut cursor = parents.get(entity).copied();
        let mut seen = std::collections::BTreeSet::new();
        while let Some(parent) = cursor {
            if !seen.insert(parent) || depth > scene.entities.len() as u32 {
                break;
            }
            depth += 1;
            cursor = parents.get(&parent).copied();
        }
        depth
    });

    let mut new_world: BTreeMap<SceneEntityId, Mat4> = BTreeMap::new();
    let mut result: BTreeMap<SceneEntityId, LocalTransform> = BTreeMap::new();
    for entity in order {
        let Some(local) = locals.get(&entity).copied() else {
            continue;
        };
        let target = targets.get(&entity).copied();
        let parent_world = parents
            .get(&entity)
            .and_then(|parent| new_world.get(parent).copied());
        // 目标世界变换：本批给了目标就用目标，否则按"父的新世界 ∘ 自己的局部"顺推，
        // 这样未选中的子实体跟着父一起动（与运行时的传播规则一致）。
        let world = match target {
            Some(target) => match target.matrix() {
                Ok(matrix) => matrix,
                Err(_) => continue,
            },
            None => {
                let Ok(matrix) = local.matrix() else { continue };
                match parent_world {
                    Some(parent) => parent * matrix,
                    None => matrix,
                }
            }
        };
        new_world.insert(entity, world);
        if target.is_some() {
            let parent_after = parents.get(&entity).copied();
            let parent_world_after =
                parent_after.and_then(|parent| new_world.get(&parent).copied());
            match local_keeping_world(parent_world_after, world) {
                Ok(local) => {
                    result.insert(entity, local);
                }
                // 剪切/镜像/退化：**拒绝**而不是近似（调用方据此提示"该变换无法在父链下表达"）。
                Err(_) => continue,
            }
        }
    }
    result
}

/// 换父后保持世界变换（docs/54 §4.1 的 keep-world 语义）。
///
/// 调用方**先**把 `parent` 改好，再把换父**之前**记下的世界矩阵传进来；本函数写回局部值。
/// 无法无损反解（剪切/镜像/退化）时返回 `Err` 且**不修改文档**——宁可让调用方拒绝这次换父，
/// 也不静默近似。`Ok(false)` = 该实体没有任何变换组件，无需写回。
pub fn keep_world_after_reparent(
    scene: &mut Scene,
    child: SceneEntityId,
    world: Mat4,
) -> Result<bool, String> {
    let parent_world = parent_world_matrix(scene, child);
    let local = local_keeping_world(parent_world, world)?;
    let Some(entity) = scene.entities.get_mut(&child) else {
        return Ok(false);
    };
    let mut changed = false;
    if entity.components.contains_key("Transform3D") {
        entity
            .components
            .insert("Transform3D".into(), encode_transform_3d(local));
        changed = true;
    }
    if entity.components.contains_key("Transform") {
        entity
            .components
            .insert("Transform".into(), encode_transform_2d(local));
        changed = true;
    }
    Ok(changed)
}

/// 2D `Transform` 组件串（与世界→局部换算配套，供编辑器写回文档用）。
#[must_use]
pub fn encode_transform_2d(local: LocalTransform) -> String {
    let transform = Transform {
        position: crate::core::transform::Vec2::new(local.translation.x, local.translation.y),
        rotation: 2.0 * local.rotation.z.atan2(local.rotation.w),
        scale: crate::core::transform::Vec2::new(local.scale.x, local.scale.y),
    };
    format!(
        "x={};y={};rotation={};scale_x={};scale_y={}",
        transform.position.x,
        transform.position.y,
        transform.rotation,
        transform.scale.x,
        transform.scale.y
    )
}

/// 3D `Transform3D` 组件串。
#[must_use]
pub fn encode_transform_3d(local: LocalTransform) -> String {
    Transform3D {
        position: local.translation,
        rotation: local.rotation,
        scale: local.scale,
    }
    .to_component_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::math3d::{Quat, Vec3};

    fn scene_with_parent_child() -> Scene {
        let mut scene = Scene::new("space");
        // docs/54 §4.1：这组用例断言的是"父子参与空间组合"，因此场景必须显式声明它；
        // 未声明的场景默认是组织关系（另一条用例 `organizational_mode_ignores_parents` 钉住）。
        scene.metadata.insert("spatial".into(), "local".into());
        let parent = SceneEntityId(1);
        let child = SceneEntityId(2);
        scene.add_entity(parent, "Parent", None).unwrap();
        scene.add_entity(child, "Child", Some(parent)).unwrap();
        scene.entities.get_mut(&parent).unwrap().components.insert(
            "Transform3D".into(),
            "position=10,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        scene.entities.get_mut(&child).unwrap().components.insert(
            "Transform3D".into(),
            "position=0,5,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        scene
    }

    /// 文档的世界变换 = 父 ∘ 局部（与运行时同一套规则）。
    #[test]
    fn document_world_transforms_compose_the_parent_chain() {
        let scene = scene_with_parent_child();
        let world = world_matrices(&scene);
        let child = world[&SceneEntityId(2)];
        assert!((child.columns[3][0] - 10.0).abs() < 1e-5);
        assert!((child.columns[3][1] - 5.0).abs() < 1e-5);
        assert_eq!(
            world_transform_3d(&scene, SceneEntityId(2))
                .expect("child world")
                .position,
            Vec3::new(10.0, 5.0, 0.0)
        );
    }

    /// Gizmo 提交的核心：给定**目标世界**变换，算出的局部值必须让世界变换正好落在目标上。
    #[test]
    fn world_targets_are_converted_back_to_local() {
        let scene = scene_with_parent_child();
        let child = SceneEntityId(2);
        let mut targets = BTreeMap::new();
        targets.insert(
            child,
            LocalTransform {
                translation: Vec3::new(20.0, 5.0, 0.0),
                rotation: Quat::identity(),
                scale: Vec3::new(1.0, 1.0, 1.0),
            },
        );
        let locals = locals_for_world_targets(&scene, &targets);
        let local = locals[&child];
        assert!(
            (local.translation.x - 10.0).abs() < 1e-4,
            "父在世界 x=10，目标世界 x=20 ⇒ 局部 x 应为 10，实际 {:?}",
            local.translation
        );

        // 写回后再解一次世界变换，必须回到目标（round-trip）。
        let mut updated = scene.clone();
        updated
            .entities
            .get_mut(&child)
            .unwrap()
            .components
            .insert("Transform3D".into(), encode_transform_3d(local));
        let world = world_matrices(&updated)[&child];
        assert!((world.columns[3][0] - 20.0).abs() < 1e-4);
        assert!((world.columns[3][1] - 5.0).abs() < 1e-4);
    }

    /// 父与子同批移动：子实体的局部值按**父的新世界**求解，不会把父的位移叠加两次。
    #[test]
    fn moving_parent_and_child_together_does_not_double_apply_the_parent_delta() {
        let scene = scene_with_parent_child();
        let parent = SceneEntityId(1);
        let child = SceneEntityId(2);
        let mut targets = BTreeMap::new();
        targets.insert(
            parent,
            LocalTransform {
                translation: Vec3::new(11.0, 0.0, 0.0),
                rotation: Quat::identity(),
                scale: Vec3::new(1.0, 1.0, 1.0),
            },
        );
        targets.insert(
            child,
            LocalTransform {
                translation: Vec3::new(11.0, 5.0, 0.0),
                rotation: Quat::identity(),
                scale: Vec3::new(1.0, 1.0, 1.0),
            },
        );
        let locals = locals_for_world_targets(&scene, &targets);
        assert!((locals[&parent].translation.x - 11.0).abs() < 1e-4);
        assert!(
            locals[&child].translation.x.abs() < 1e-4,
            "子实体相对父的局部 x 应保持 0，实际 {:?}",
            locals[&child].translation
        );
    }

    /// 自环与互环都不参与父子组合（按无父处理），不会让整份文档失去世界变换。
    #[test]
    fn self_and_mutual_parent_cycles_fall_back_to_local() {
        let mut scene = Scene::new("broken");
        let lone = SceneEntityId(1);
        scene.add_entity(lone, "Lone", None).unwrap();
        scene.entities.get_mut(&lone).unwrap().components.insert(
            "Transform3D".into(),
            "position=3,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        // 自环：`Scene::add_entity` 不允许，直接写 `parent` 模拟手改过的场景文件。
        scene.entities.get_mut(&lone).unwrap().parent = Some(lone);

        let a = SceneEntityId(2);
        let b = SceneEntityId(3);
        scene.add_entity(a, "A", None).unwrap();
        scene.add_entity(b, "B", Some(a)).unwrap();
        for (id, x) in [(a, 4.0_f32), (b, 6.0)] {
            scene.entities.get_mut(&id).unwrap().components.insert(
                "Transform3D".into(),
                format!("position={x},0,0;rotation=0,0,0,1;scale=1,1,1"),
            );
        }
        // 互环：A 的父改成 B。
        scene.entities.get_mut(&a).unwrap().parent = Some(b);

        let world = world_matrices(&scene);
        assert!(
            (world[&lone].columns[3][0] - 3.0).abs() < 1e-5,
            "自环退回局部值"
        );
        assert!(
            (world[&a].columns[3][0] - 4.0).abs() < 1e-5
                && (world[&b].columns[3][0] - 6.0).abs() < 1e-5,
            "互环也各自退回局部值"
        );
    }

    /// 换父 keep-world：改好 parent 之后写回局部值，世界位置不变。
    #[test]
    fn reparent_keeps_the_world_transform() {
        let mut scene = scene_with_parent_child();
        let child = SceneEntityId(2);
        let before = world_matrices(&scene)[&child];
        let new_parent = SceneEntityId(3);
        scene.add_entity(new_parent, "Other", None).unwrap();
        scene
            .entities
            .get_mut(&new_parent)
            .unwrap()
            .components
            .insert(
                "Transform3D".into(),
                "position=-4,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        scene.entities.get_mut(&child).unwrap().parent = Some(new_parent);
        assert!(
            keep_world_after_reparent(&mut scene, child, before).expect("keep-world 可行"),
            "该实体有 Transform3D，必须真的写回"
        );
        let after = world_matrices(&scene)[&child];
        for column in 0..4 {
            for row in 0..4 {
                assert!(
                    (before.columns[column][row] - after.columns[column][row]).abs() < 1e-4,
                    "换父后世界矩阵必须不变：{before:?} vs {after:?}"
                );
            }
        }
    }

    /// docs/54 §4.1：**未声明**（v2 默认）时 `parent` 只是组织关系——文档侧也不能组合。
    ///
    /// 文档与运行时必须由同一份声明决定，否则编辑器手柄会框在"组合后"的位置、而画面画在局部
    /// 位置（§8-⑦ 修掉的正是这一类分裂）。
    #[test]
    fn organizational_mode_ignores_parents_on_both_sides() {
        let mut scene = scene_with_parent_child();
        scene.metadata.remove("spatial");
        assert_eq!(spatial_mode(&scene), SpatialMode::Organizational);
        assert!(parent_world_matrix(&scene, SceneEntityId(2)).is_none());
        let child = world_matrices(&scene)[&SceneEntityId(2)];
        assert!(
            (child.columns[3][0]).abs() < 1e-5 && (child.columns[3][1] - 5.0).abs() < 1e-5,
            "组织关系下子实体的世界矩阵就是它自己的局部矩阵（0, 5）：{child:?}"
        );
        assert_eq!(
            world_transform_3d(&scene, SceneEntityId(2))
                .expect("child local")
                .position,
            Vec3::new(0.0, 5.0, 0.0)
        );
        // 显式声明同一件事与未声明等价。
        scene
            .metadata
            .insert("spatial".into(), "organizational".into());
        assert_eq!(world_matrices(&scene)[&SceneEntityId(2)].columns[3][0], 0.0);
    }

    /// 2D 平面视图：平面层级给出世界位置，非平面（含 z 位移）**不硬压平**。
    #[test]
    fn plane_view_is_offered_only_for_planar_hierarchies() {
        let mut scene = Scene::new("plane");
        scene.metadata.insert("spatial".into(), "local".into());
        let parent = SceneEntityId(1);
        let child = SceneEntityId(2);
        scene.add_entity(parent, "Parent", None).unwrap();
        scene.add_entity(child, "Child", Some(parent)).unwrap();
        for (id, x) in [(parent, 100.0_f32), (child, 0.0)] {
            scene.entities.get_mut(&id).unwrap().components.insert(
                "Transform".into(),
                format!("x={x};y=0;rotation=0;scale_x=1;scale_y=1"),
            );
        }
        assert_eq!(
            world_transform_2d(&scene, child)
                .expect("planar child")
                .position,
            crate::core::transform::Vec2::new(100.0, 0.0)
        );

        // 父改成沿 z 抬高：2D 视图必须消失（不接受压平）。
        scene.entities.get_mut(&parent).unwrap().components.insert(
            "Transform3D".into(),
            "position=100,0,5;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        assert!(
            world_transform_2d(&scene, child).is_none(),
            "含 z 位移的父子组合不得被硬压平成 2D"
        );
    }
}
