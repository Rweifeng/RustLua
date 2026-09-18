//! WP2：统一的局部/世界变换、TRS 反解与父先序传播。

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use crate::core::{
    math3d::{Mat4, Math3dError, Quat, Vec3},
    scene::{Scene, SceneEntityId},
    transform::Transform,
    transform::Transform3D,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalTransform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl LocalTransform {
    pub const fn identity() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::identity(),
            scale: Vec3::new(1.0, 1.0, 1.0),
        }
    }

    pub fn from_2d(value: Transform) -> Result<Self, Math3dError> {
        Ok(Self {
            translation: Vec3::new(value.position.x, value.position.y, 0.0),
            rotation: Quat::from_axis_angle(Vec3::Z, value.rotation)?,
            scale: Vec3::new(value.scale.x, value.scale.y, 1.0),
        })
    }

    pub const fn from_3d(value: Transform3D) -> Self {
        Self {
            translation: value.position,
            rotation: value.rotation,
            scale: value.scale,
        }
    }

    pub fn matrix(self) -> Result<Mat4, Math3dError> {
        if !self.translation.is_finite()
            || !self.scale.is_finite()
            || self.scale.x <= 0.0
            || self.scale.y <= 0.0
            || self.scale.z <= 0.0
            || !self.rotation.is_unit()
        {
            return Err(Math3dError::NonFinite);
        }
        Ok(Mat4::translation(self.translation)
            * Mat4::from_quat(self.rotation)
            * Mat4::scale(self.scale))
    }

    /// TRS 反解。含剪切或镜像的矩阵**明确拒绝**而不是近似——非均匀父缩放与子旋转
    /// 组合会产生剪切，静默近似会让换父/迁移悄悄改变画面（docs/38 §3.1）。
    pub fn from_matrix(matrix: Mat4) -> Result<Self, DecomposeError> {
        // 透视项非 (0,0,0,1) 的不是仿射变换，不可能是 TRS。
        if matrix.columns[0][3] != 0.0
            || matrix.columns[1][3] != 0.0
            || matrix.columns[2][3] != 0.0
            || matrix.columns[3][3] != 1.0
        {
            return Err(DecomposeError::NotAffine);
        }
        let translation = Vec3::new(
            matrix.columns[3][0],
            matrix.columns[3][1],
            matrix.columns[3][2],
        );
        let c0 = Vec3::new(
            matrix.columns[0][0],
            matrix.columns[0][1],
            matrix.columns[0][2],
        );
        let c1 = Vec3::new(
            matrix.columns[1][0],
            matrix.columns[1][1],
            matrix.columns[1][2],
        );
        let c2 = Vec3::new(
            matrix.columns[2][0],
            matrix.columns[2][1],
            matrix.columns[2][2],
        );
        if !translation.is_finite() || !c0.is_finite() || !c1.is_finite() || !c2.is_finite() {
            return Err(DecomposeError::NotAffine);
        }

        // Gram–Schmidt：先取出三个缩放，再判断剩下的正交化残差（剪切）是否可忽略。
        let scale_x = c0.length();
        if scale_x <= MIN_SCALE {
            return Err(DecomposeError::DegenerateScale);
        }
        let axis_x = c0 / scale_x;
        let shear_xy = axis_x.dot(c1);
        let c1_ortho = c1 - axis_x * shear_xy;
        let scale_y = c1_ortho.length();
        if scale_y <= MIN_SCALE {
            return Err(DecomposeError::DegenerateScale);
        }
        let axis_y = c1_ortho / scale_y;
        let shear_xz = axis_x.dot(c2);
        let shear_yz = axis_y.dot(c2);
        let c2_ortho = c2 - axis_x * shear_xz - axis_y * shear_yz;
        let scale_z = c2_ortho.length();
        if scale_z <= MIN_SCALE {
            return Err(DecomposeError::DegenerateScale);
        }
        let axis_z = c2_ortho / scale_z;

        // 行列式为负 = 镜像（负缩放）。负缩放按 docs/38 §3.1 另立任务，这里拒绝，
        // 不偷偷翻一个轴把镜像吞掉。
        if axis_x.dot(axis_y.cross(axis_z)) <= 0.0 {
            return Err(DecomposeError::Mirrored);
        }

        // 相对剪切：绝对阈值会在大缩放下误报。
        let scale_ref = scale_x.max(scale_y).max(scale_z);
        let shear = shear_xy.abs().max(shear_xz.abs()).max(shear_yz.abs());
        if shear > SHEAR_TOLERANCE * scale_ref {
            return Err(DecomposeError::Shear { amount: shear });
        }

        Ok(Self {
            translation,
            rotation: quat_from_axes(axis_x, axis_y, axis_z),
            scale: Vec3::new(scale_x, scale_y, scale_z),
        })
    }
}

/// 缩放下限：低于它无法可靠地归一化出旋转基。
const MIN_SCALE: f32 = 1e-6;
/// 相对剪切容差。float 累乘误差在深度 100 的链上会被放大，取 1e-4 留出余量。
const SHEAR_TOLERANCE: f32 = 1e-4;

/// 由三个正交单位轴（矩阵的列）构造旋转四元数。取迹最大分支，避免除以近零值。
fn quat_from_axes(x: Vec3, y: Vec3, z: Vec3) -> Quat {
    let trace = x.x + y.y + z.z;
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        Quat {
            x: (y.z - z.y) / s,
            y: (z.x - x.z) / s,
            z: (x.y - y.x) / s,
            w: 0.25 * s,
        }
    } else if x.x > y.y && x.x > z.z {
        let s = (1.0 + x.x - y.y - z.z).sqrt() * 2.0;
        Quat {
            x: 0.25 * s,
            y: (y.x + x.y) / s,
            z: (z.x + x.z) / s,
            w: (y.z - z.y) / s,
        }
    } else if y.y > z.z {
        let s = (1.0 + y.y - x.x - z.z).sqrt() * 2.0;
        Quat {
            x: (y.x + x.y) / s,
            y: 0.25 * s,
            z: (z.y + y.z) / s,
            w: (z.x - x.z) / s,
        }
    } else {
        let s = (1.0 + z.z - x.x - y.y).sqrt() * 2.0;
        Quat {
            x: (z.x + x.z) / s,
            y: (z.y + y.z) / s,
            z: 0.25 * s,
            w: (x.y - y.x) / s,
        }
    }
}

impl Default for LocalTransform {
    fn default() -> Self {
        Self::identity()
    }
}

/// 派生出来的**世界变换**（docs/54 §8-②）：唯一真源，由统一传播系统写入。
///
/// 存的是**矩阵**而不是"2D/3D 各一份组件"：世界变换只有一个，2D 与 3D 只是它的两种视图
/// （`plane` / `space`）。此前 2D/3D 各有自己的派生组件（`WorldTransform` /
/// `WorldTransform3D`），"谁是权威"取决于消费端读哪一个；现在两者都是同一份矩阵的视图。
///
/// 视图字段是 `Option`：剪切/镜像这类组合**矩阵本身完全正确**（`Global(child) =
/// Global(parent) * Local(child)`），但压不成 TRS。此时视图退回局部值并记诊断，而矩阵仍是真相
/// ——提取路径可以直接用矩阵，不必因为"压不平"而丢掉世界变换。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalTransform {
    matrix: Mat4,
    /// 平面（2D）视图：该实体有 2D 局部变换时一定有值（压不平时退回局部值）。
    plane: Option<Transform>,
    /// 空间（3D）视图：该实体有 3D 局部变换时一定有值。
    space: Option<Transform3D>,
}

impl GlobalTransform {
    /// 由矩阵构造（没有视图：纯数学用途，例如迁移与换父）。
    #[must_use]
    pub const fn from_matrix(matrix: Mat4) -> Self {
        Self {
            matrix,
            plane: None,
            space: None,
        }
    }

    /// 同时给出两种视图（传播使用；视图为 `None` 表示该实体没有对应空间的局部变换）。
    #[must_use]
    pub const fn with_views(
        matrix: Mat4,
        plane: Option<Transform>,
        space: Option<Transform3D>,
    ) -> Self {
        Self {
            matrix,
            plane,
            space,
        }
    }

    /// 世界矩阵——派生真源。
    #[must_use]
    pub const fn matrix(self) -> Mat4 {
        self.matrix
    }

    /// 平面（2D）视图。
    #[must_use]
    pub const fn plane(self) -> Option<Transform> {
        self.plane
    }

    /// 空间（3D）视图。
    #[must_use]
    pub const fn space(self) -> Option<Transform3D> {
        self.space
    }

    /// 求相对给定父世界变换的局部变换——"换父保持世界位置"的核心一步。
    pub fn local_relative_to(
        self,
        parent: Option<GlobalTransform>,
    ) -> Result<LocalTransform, DecomposeError> {
        let relative = match parent {
            Some(parent) => {
                parent
                    .matrix
                    .inverse()
                    .map_err(|_| DecomposeError::NotInvertible)?
                    * self.matrix
            }
            None => self.matrix,
        };
        LocalTransform::from_matrix(relative)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DecomposeError {
    /// 非仿射（含透视项）或含非有限值。
    NotAffine,
    /// 缩放退化到无法归一化。
    DegenerateScale,
    /// 含剪切：无法无损表达为 TRS。
    Shear { amount: f32 },
    /// 镜像/负缩放（行列式为负）。
    Mirrored,
    /// 矩阵奇异，求逆失败。
    NotInvertible,
}

impl fmt::Display for DecomposeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAffine => write!(formatter, "matrix is not an affine transform"),
            Self::DegenerateScale => write!(formatter, "matrix has a degenerate scale"),
            Self::Shear { amount } => write!(formatter, "matrix contains shear ({amount})"),
            Self::Mirrored => write!(formatter, "matrix mirrors (negative scale)"),
            Self::NotInvertible => write!(formatter, "matrix is singular"),
        }
    }
}

impl std::error::Error for DecomposeError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PropagationError {
    MissingParent {
        entity: SceneEntityId,
        parent: SceneEntityId,
    },
    Cycle(SceneEntityId),
    InvalidLocal(SceneEntityId),
}

impl fmt::Display for PropagationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingParent { entity, parent } => {
                write!(formatter, "entity {} missing parent {}", entity.0, parent.0)
            }
            Self::Cycle(entity) => write!(formatter, "transform parent cycle at {}", entity.0),
            Self::InvalidLocal(entity) => {
                write!(formatter, "invalid local transform at {}", entity.0)
            }
        }
    }
}

impl std::error::Error for PropagationError {}

#[derive(Clone, Debug, PartialEq)]
pub enum ReparentError {
    Propagation(PropagationError),
    Decompose(DecomposeError),
    /// 新父是自己或自己的后代，换父会成环。
    WouldCycle(SceneEntityId),
}

impl fmt::Display for ReparentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Propagation(error) => error.fmt(formatter),
            Self::Decompose(error) => error.fmt(formatter),
            Self::WouldCycle(entity) => {
                write!(
                    formatter,
                    "reparenting would create a cycle at {}",
                    entity.0
                )
            }
        }
    }
}

impl std::error::Error for ReparentError {}

impl From<PropagationError> for ReparentError {
    fn from(error: PropagationError) -> Self {
        Self::Propagation(error)
    }
}

impl From<DecomposeError> for ReparentError {
    fn from(error: DecomposeError) -> Self {
        Self::Decompose(error)
    }
}

/// 父先序遍历里"这条数据为什么解不了"的分类（docs/54 §8-②）。
///
/// 与 [`PropagationError`] 的区别：这是**诊断口径**（运行时逐条报告后继续跑），
/// 而 `PropagationError` 是**拒绝口径**（加载期直接失败）。两者由同一份遍历得出，
/// 因此"什么算坏数据"只有一处定义。
///
/// `Id` 是实体标识类型：文档侧是 [`SceneEntityId`]，运行时侧是
/// [`crate::core::entity::EntityId`]——驱动本身对两者都成立，因此不需要两份遍历。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropagationProblem<Id> {
    /// 父 id 根本不在实体集合里（手改过的场景可能出现）。**本实体仍按链根求解**，
    /// 只是这条断链必须被点名——静默当成"没有父"会让子树的位移看起来毫无原因。
    MissingParent { entity: Id, parent: Id },
    /// 父在集合里，但父自己没有局部变换：它贡献不了任何变换，**本实体与它的后代都不参与求解**
    /// （各自保留局部值）。此前这种数据会被报成 `Cycle`，让作者去找一个根本不存在的环。
    ParentWithoutLocal { entity: Id, parent: Id },
    /// 环上的全体成员（顺序确定：从进入环的那个实体开始）。
    Cycle(Vec<Id>),
    /// 祖先链上有解不了的实体（环 / 父无局部变换）：本实体不参与求解，保留局部值。
    UnresolvableAncestor { entity: Id, ancestor: Id },
}

/// 父先序遍历的**唯一驱动**：产出自顶向下可安全求解的顺序，外加逐条的坏数据分类。
///
/// 为什么要抽出来：运行时传播（[`crate::core::hierarchy::propagate_spatial`]，容错：保留局部值
/// 并逐条诊断）与加载期求解（[`propagate_all`]，严格：直接报错）此前**各写了一遍**父链上溯、
/// 成环判定与破坏传播，于是同一份坏数据在两条路上会给出不同说法（例如"父没有变换"在运行时被
/// 报成环）。现在两者都调用这里，"谁先算""什么算坏"只有一份实现，差别只剩**遇到问题怎么办**。
///
/// 语义（确定性，与实体 id 无关）：
/// - 缺父 ⇒ 该实体按**链根**求解（世界 = 自己的局部）并报 [`PropagationProblem::MissingParent`]；
/// - 父无局部变换 ⇒ 该实体与其**后代**都不参与求解（保留局部值）；
/// - 成环 ⇒ 环上成员与其**后代**都不参与求解（环外的实体照常求解）。
///
/// `ordered` 满足"父一定排在子之前"，因此调用方可以一遍线性求解，不需要再自己判环。
#[must_use]
pub fn parent_first_plan<Id: Copy + Ord>(
    ids: impl IntoIterator<Item = Id>,
    parent_of: impl Fn(Id) -> Option<Id>,
    has_local: impl Fn(Id) -> bool,
) -> ParentFirstPlan<Id> {
    let ids: BTreeSet<Id> = ids.into_iter().collect();
    let mut ordered = Vec::with_capacity(ids.len());
    let mut problems = Vec::new();
    // 0 = 未访问，1 = 当前链上，2 = 已求解，3 = 不可解。
    let mut state: BTreeMap<Id, u8> = BTreeMap::new();
    for &start in &ids {
        if state.get(&start).copied().unwrap_or(0) != 0 {
            continue;
        }
        // 沿父链上溯；`chain` 从 start 一路指向上游。
        let mut chain: Vec<Id> = Vec::new();
        let mut cursor = Some(start);
        // 本次链里第一个"解不了"的实体（环成员或父无局部变换者），以及它的原因。
        let mut unresolved_from: Option<(usize, Id)> = None;
        while let Some(entity) = cursor {
            match state.get(&entity).copied().unwrap_or(0) {
                2 => break, // 上游已求解：chain 里剩下的全都能排
                3 => {
                    // 上游已知不可解：chain 里**全部**都是它的后代。
                    unresolved_from = Some((chain.len(), entity));
                    break;
                }
                1 => {
                    // 成环：从 entity 在 chain 里的位置到链尾都是环成员。
                    let position = chain
                        .iter()
                        .position(|candidate| *candidate == entity)
                        .unwrap_or(0);
                    let members: Vec<Id> = chain[position..].to_vec();
                    for member in &members {
                        state.insert(*member, 3);
                    }
                    problems.push(PropagationProblem::Cycle(members.clone()));
                    // 环**上方**的那一段（更靠近 start）是环的后代：它们同样不参与求解。
                    chain.truncate(position);
                    unresolved_from = Some((chain.len(), members[0]));
                    break;
                }
                _ => {}
            }
            state.insert(entity, 1);
            // 链根判定：没有父，或父不在集合里（后一种要报告，但仍按链根求解）。
            let Some(parent) = parent_of(entity) else {
                chain.push(entity);
                break;
            };
            if !ids.contains(&parent) {
                problems.push(PropagationProblem::MissingParent { entity, parent });
                chain.push(entity);
                break;
            }
            if !has_local(parent) {
                problems.push(PropagationProblem::ParentWithoutLocal { entity, parent });
                chain.push(entity);
                unresolved_from = Some((chain.len() - 1, entity));
                break;
            }
            chain.push(entity);
            cursor = Some(parent);
        }
        match unresolved_from {
            Some((from, ancestor)) => {
                for (index, entity) in chain.iter().enumerate() {
                    if state.get(entity).copied() == Some(3) {
                        continue; // 环成员已经报过
                    }
                    state.insert(*entity, 3);
                    if index < from {
                        problems.push(PropagationProblem::UnresolvableAncestor {
                            entity: *entity,
                            ancestor,
                        });
                    }
                }
            }
            None => {
                // 出栈顺序 = 上游在前（`chain` 是从 start 往上走的）。
                for entity in chain.iter().rev() {
                    if state.get(entity).copied() == Some(1) {
                        state.insert(*entity, 2);
                        ordered.push(*entity);
                    }
                }
            }
        }
    }
    ParentFirstPlan { ordered, problems }
}

/// [`parent_first_plan`] 的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParentFirstPlan<Id> {
    /// 父一定排在子之前的求解顺序；只包含祖先链完整可解的实体。
    pub ordered: Vec<Id>,
    /// 坏数据，逐条分类（顺序确定）。
    pub problems: Vec<PropagationProblem<Id>>,
}

/// 全量父先序求解：正确性参照实现。增量路径的结果必须与它逐实体一致。
///
/// 遍历本身走 [`parent_first_plan`]（与运行时传播同一份驱动）；任何坏数据都按
/// [`PropagationError`] **拒绝**，因此加载期的语义是"要么整棵树可解，要么明确失败"。
pub fn propagate_all(
    locals: &BTreeMap<SceneEntityId, LocalTransform>,
    parents: &BTreeMap<SceneEntityId, SceneEntityId>,
) -> Result<BTreeMap<SceneEntityId, GlobalTransform>, PropagationError> {
    let plan = parent_first_plan(
        locals.keys().copied(),
        |entity| parents.get(&entity).copied(),
        |entity| locals.contains_key(&entity),
    );
    if let Some(problem) = plan.problems.first() {
        return Err(match problem {
            PropagationProblem::MissingParent { entity, parent }
            | PropagationProblem::ParentWithoutLocal { entity, parent } => {
                PropagationError::MissingParent {
                    entity: *entity,
                    parent: *parent,
                }
            }
            PropagationProblem::Cycle(members) => {
                PropagationError::Cycle(members.first().copied().unwrap_or(SceneEntityId(0)))
            }
            PropagationProblem::UnresolvableAncestor { entity, .. } => {
                PropagationError::InvalidLocal(*entity)
            }
        });
    }
    let mut globals: BTreeMap<SceneEntityId, GlobalTransform> = BTreeMap::new();
    for entity in plan.ordered {
        let local = locals
            .get(&entity)
            .ok_or(PropagationError::InvalidLocal(entity))?
            .matrix()
            .map_err(|_| PropagationError::InvalidLocal(entity))?;
        let global = match parents.get(&entity).copied() {
            Some(parent) => {
                let parent_global = globals
                    .get(&parent)
                    .ok_or(PropagationError::MissingParent { entity, parent })?;
                parent_global.matrix() * local
            }
            None => local,
        };
        globals.insert(entity, GlobalTransform::from_matrix(global));
    }
    Ok(globals)
}

/// 带脏标记的层级变换树：只有局部值变动的子树才重算世界矩阵。
///
/// 与"每帧全量传播"的区别由 `recomputes()` 计数直接体现；正确性以
/// [`propagate_all`] 为参照，两者必须逐实体一致。
#[derive(Debug, Default)]
pub struct TransformTree {
    locals: BTreeMap<SceneEntityId, LocalTransform>,
    parents: BTreeMap<SceneEntityId, SceneEntityId>,
    children: BTreeMap<SceneEntityId, BTreeSet<SceneEntityId>>,
    globals: BTreeMap<SceneEntityId, GlobalTransform>,
    dirty: BTreeSet<SceneEntityId>,
    recomputes: u64,
}

impl TransformTree {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.locals.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.locals.is_empty()
    }

    /// 本次统计窗口内重算过的世界矩阵个数。
    #[must_use]
    pub fn recomputes(&self) -> u64 {
        self.recomputes
    }

    pub fn reset_recomputes(&mut self) {
        self.recomputes = 0;
    }

    #[must_use]
    pub fn local(&self, entity: SceneEntityId) -> Option<LocalTransform> {
        self.locals.get(&entity).copied()
    }

    pub fn set_local(&mut self, entity: SceneEntityId, local: LocalTransform) {
        self.locals.insert(entity, local);
        self.mark_subtree_dirty(entity);
    }

    pub fn set_parent(
        &mut self,
        entity: SceneEntityId,
        parent: Option<SceneEntityId>,
    ) -> Result<(), PropagationError> {
        if let Some(parent) = parent
            && (parent == entity || self.is_descendant_of(parent, entity))
        {
            return Err(PropagationError::Cycle(entity));
        }
        if let Some(old) = self.parents.remove(&entity)
            && let Some(siblings) = self.children.get_mut(&old)
        {
            siblings.remove(&entity);
        }
        if let Some(parent) = parent {
            self.parents.insert(entity, parent);
            self.children.entry(parent).or_default().insert(entity);
        }
        self.mark_subtree_dirty(entity);
        Ok(())
    }

    pub fn remove(&mut self, entity: SceneEntityId) {
        self.locals.remove(&entity);
        self.globals.remove(&entity);
        self.dirty.remove(&entity);
        if let Some(old) = self.parents.remove(&entity)
            && let Some(siblings) = self.children.get_mut(&old)
        {
            siblings.remove(&entity);
        }
        self.children.remove(&entity);
    }

    /// 取世界矩阵：只沿父链补算脏节点，干净的祖先直接命中缓存。
    pub fn global(&mut self, entity: SceneEntityId) -> Result<GlobalTransform, PropagationError> {
        if let Some(global) = self.cached(entity) {
            return Ok(global);
        }
        // 先向上收集未命中的链，再自顶向下重算：避免递归借用冲突，也天然完成环检测。
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut cursor = Some(entity);
        while let Some(current) = cursor {
            if let Some(global) = self.cached(current) {
                if current == entity {
                    return Ok(global);
                }
                break;
            }
            if !seen.insert(current) {
                return Err(PropagationError::Cycle(current));
            }
            chain.push(current);
            cursor = match self.parents.get(&current).copied() {
                Some(parent) => {
                    if !self.locals.contains_key(&parent) {
                        return Err(PropagationError::MissingParent {
                            entity: current,
                            parent,
                        });
                    }
                    Some(parent)
                }
                None => None,
            };
        }
        chain.reverse();
        for current in chain {
            let local = self
                .locals
                .get(&current)
                .ok_or(PropagationError::InvalidLocal(current))?
                .matrix()
                .map_err(|_| PropagationError::InvalidLocal(current))?;
            let global = match self.parents.get(&current).copied() {
                Some(parent) => {
                    self.globals
                        .get(&parent)
                        .copied()
                        .ok_or(PropagationError::MissingParent {
                            entity: current,
                            parent,
                        })?
                        .matrix()
                        * local
                }
                None => local,
            };
            self.globals
                .insert(current, GlobalTransform::from_matrix(global));
            self.dirty.remove(&current);
            self.recomputes += 1;
        }
        self.cached(entity)
            .ok_or(PropagationError::InvalidLocal(entity))
    }

    /// 世界位置（平移分量）。便捷入口，2D/3D 共用。
    pub fn world_position(&mut self, entity: SceneEntityId) -> Result<Vec3, PropagationError> {
        let global = self.global(entity)?.matrix();
        Ok(Vec3::new(
            global.columns[3][0],
            global.columns[3][1],
            global.columns[3][2],
        ))
    }

    /// 全量重算（清空缓存）：正确性参照与"全量 vs 增量"对照用。
    pub fn recompute_all(&mut self) -> Result<(), PropagationError> {
        self.globals.clear();
        self.dirty = self.locals.keys().copied().collect();
        for entity in self.locals.keys().copied().collect::<Vec<_>>() {
            self.global(entity)?;
        }
        Ok(())
    }

    /// 换父并保持世界位置。目标局部变换无法用 TRS 无损表达时**拒绝**，不改动树。
    pub fn reparent_keep_world(
        &mut self,
        entity: SceneEntityId,
        parent: Option<SceneEntityId>,
    ) -> Result<LocalTransform, ReparentError> {
        if let Some(parent) = parent
            && (parent == entity || self.is_descendant_of(parent, entity))
        {
            return Err(ReparentError::WouldCycle(parent));
        }
        let world = self.global(entity)?;
        let parent_world = match parent {
            Some(parent) => Some(self.global(parent)?),
            None => None,
        };
        let local = world.local_relative_to(parent_world)?;
        self.set_parent(entity, parent)?;
        self.set_local(entity, local);
        Ok(local)
    }

    fn cached(&self, entity: SceneEntityId) -> Option<GlobalTransform> {
        if self.dirty.contains(&entity) {
            return None;
        }
        self.globals.get(&entity).copied()
    }

    fn mark_subtree_dirty(&mut self, entity: SceneEntityId) {
        let mut stack = vec![entity];
        while let Some(current) = stack.pop() {
            // 已是脏的子树不必再走一遍：它的后代必然也脏。
            if !self.dirty.insert(current) {
                continue;
            }
            if let Some(children) = self.children.get(&current) {
                stack.extend(children.iter().copied());
            }
        }
    }

    fn is_descendant_of(&self, candidate: SceneEntityId, ancestor: SceneEntityId) -> bool {
        let mut seen = BTreeSet::new();
        let mut cursor = Some(candidate);
        while let Some(entity) = cursor {
            if !seen.insert(entity) {
                return false;
            }
            if entity == ancestor {
                return true;
            }
            cursor = self.parents.get(&entity).copied();
        }
        false
    }
}

/// 迁移拒绝原因：无法无损建立空间继承的实体逐条上报，不做近似。
#[derive(Clone, Debug, PartialEq)]
pub enum MigrationRefusal {
    /// 新局部变换无法无损表达为 TRS（剪切/镜像/退化/奇异）。
    NotRepresentable(DecomposeError),
    /// 父链成环，无法建立继承。
    Cycle(SceneEntityId),
}

/// 迁移报告。`refused` 非空即表示该场景**不能**无损迁移。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MigrationReport {
    /// 已建立空间继承并保持世界位置的实体。
    pub converted: Vec<SceneEntityId>,
    /// 本来就是根（无父），局部值不变。
    pub roots: Vec<SceneEntityId>,
    /// 被拒绝的实体：保持为根，世界位置因此仍然不变。
    pub refused: Vec<(SceneEntityId, MigrationRefusal)>,
    /// 不参与空间迁移的实体（无有效 `Transform3D`，或父节点没有）。
    pub skipped: Vec<SceneEntityId>,
}

impl MigrationReport {
    #[must_use]
    pub fn is_lossless(&self) -> bool {
        self.refused.is_empty()
    }
}

/// v2 → 空间层级的**显式**迁移，保持每个实体的世界位置不变。
///
/// v2 的 `parent` 只是组织关系：提取器按**单实体** TRS 构造世界矩阵，父子之间没有空间
/// 继承。因此迁移必须把"父世界矩阵的逆"烘进子实体的新局部变换，而不是把旧局部值直接
/// 当作相对父节点的坐标——后者会让整棵子树的画面位移（docs/40 §3 WP2 明确禁止）。
///
/// 无法无损表达为 TRS 的实体**拒绝迁移**并逐条报告，且保持为根，世界位置同样不变。
/// 本函数只读输入、只返回新数据（dry-run 语义），写回由调用方决定（WP9）。
pub fn migrate_v2_hierarchy(scene: &Scene) -> (TransformTree, MigrationReport) {
    let mut report = MigrationReport::default();
    let mut tree = TransformTree::new();
    // v2 下每个实体的世界矩阵就是它自己的 TRS——这是"保持世界位置"的基准。
    // 2D（`Transform`）与 3D（`Transform3D`）都算进来：负责人裁定"所有场景 `parent` 一律按
    // 局部空间解释"，而真实场景里 2D 实体占多数，只看 3D 会把它们全当成"无变换"跳过。
    let mut world = BTreeMap::new();
    for entity in scene.entities.values() {
        let Some((_, local)) = scene_entity_local(entity) else {
            report.skipped.push(entity.id);
            continue;
        };
        match local.matrix() {
            Ok(matrix) => {
                world.insert(entity.id, matrix);
                tree.set_local(entity.id, local);
            }
            Err(_) => report.skipped.push(entity.id),
        }
    }
    for entity in scene.entities.values() {
        let Some(&own_world) = world.get(&entity.id) else {
            continue;
        };
        let Some(parent) = entity.parent else {
            report.roots.push(entity.id);
            continue;
        };
        let Some(&parent_world) = world.get(&parent) else {
            // 父节点没有有效 Transform3D：保持组织关系，不建立空间继承，世界位置不变。
            report.skipped.push(entity.id);
            continue;
        };
        if let Err(error) = tree.set_parent(entity.id, Some(parent)) {
            report
                .refused
                .push((entity.id, MigrationRefusal::Cycle(entity.id)));
            let _ = error;
            continue;
        }
        match GlobalTransform::from_matrix(own_world)
            .local_relative_to(Some(GlobalTransform::from_matrix(parent_world)))
        {
            Ok(local) => {
                tree.set_local(entity.id, local);
                report.converted.push(entity.id);
            }
            Err(error) => {
                // 拒绝：退回根，世界位置仍然等于原来的 TRS。
                tree.set_parent(entity.id, None)
                    .expect("detaching cannot fail");
                report
                    .refused
                    .push((entity.id, MigrationRefusal::NotRepresentable(error)));
            }
        }
    }
    (tree, report)
}

/// 迁移后应当写回场景的**组件串**（2D `Transform` 与 3D `Transform3D`），以及逐条报告。
///
/// 与 [`migrate_v2_hierarchy`] 的区别：那个只给 3D（`Transform3D`）建树，而真实场景里
/// **2D 实体占多数**；负责人裁定"所有场景 `parent` 一律按局部空间解释"，因此 2D 必须一起迁移。
/// 本函数把两种空间都算进去，并直接给出可写回的组件串——写盘由调用方（WP9 迁移命令）决定，
/// 这里保持 dry-run 语义（只读输入、只返回新数据）。
pub fn migrate_scene_to_local_space(scene: &Scene) -> (SpatialMigration, MigrationReport) {
    let mut report = MigrationReport::default();
    let mut migrated = SpatialMigration::default();
    // v2 下每个实体的世界矩阵就是它自己的 TRS（局部=世界），这是"保持世界位置"的基准。
    let mut world: BTreeMap<SceneEntityId, Mat4> = BTreeMap::new();
    let mut spaces: BTreeMap<SceneEntityId, SpatialKind> = BTreeMap::new();
    for entity in scene.entities.values() {
        match scene_entity_local(entity) {
            Some((kind, local)) => match local.matrix() {
                Ok(matrix) => {
                    world.insert(entity.id, matrix);
                    spaces.insert(entity.id, kind);
                }
                Err(_) => report.skipped.push(entity.id),
            },
            None => report.skipped.push(entity.id),
        }
    }
    for entity in scene.entities.values() {
        let Some(&own_world) = world.get(&entity.id) else {
            continue;
        };
        let kind = spaces[&entity.id];
        // 保持世界位置的两种结果：建继承（局部 = 父世界的逆 × 自己世界）或保持为根。
        let keep_as_root =
            |migrated: &mut SpatialMigration, report: &mut MigrationReport| match encode_local(
                kind, own_world,
            ) {
                Some(encoded) => {
                    migrated.locals.insert(entity.id, encoded);
                }
                None => report.skipped.push(entity.id),
            };
        let Some(parent) = entity.parent else {
            keep_as_root(&mut migrated, &mut report);
            report.roots.push(entity.id);
            continue;
        };
        let Some(&parent_world) = world.get(&parent) else {
            // 父节点没有可用的变换：保持组织关系但不建立空间继承，世界位置不变。
            keep_as_root(&mut migrated, &mut report);
            report.skipped.push(entity.id);
            continue;
        };
        match GlobalTransform::from_matrix(own_world)
            .local_relative_to(Some(GlobalTransform::from_matrix(parent_world)))
        {
            Ok(local) => match encode_local_checked(kind, local) {
                Some(encoded) => {
                    migrated.locals.insert(entity.id, encoded);
                    report.converted.push(entity.id);
                }
                None => {
                    // 拒绝（含 2D 压不平）：保持为根，并**清掉 parent**，否则新语义下世界位置会变。
                    keep_as_root(&mut migrated, &mut report);
                    report.refused.push((
                        entity.id,
                        MigrationRefusal::NotRepresentable(DecomposeError::Shear { amount: 0.0 }),
                    ));
                    migrated.detached.push(entity.id);
                }
            },
            Err(error) => {
                keep_as_root(&mut migrated, &mut report);
                report
                    .refused
                    .push((entity.id, MigrationRefusal::NotRepresentable(error)));
                migrated.detached.push(entity.id);
            }
        }
    }
    (migrated, report)
}

/// 实体属于哪种空间（用于选择写回的组件串格式）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpatialKind {
    TwoD,
    ThreeD,
}

/// 迁移结果：每个实体应写回的组件串 + 需要**清掉 `parent`** 的实体。
///
/// `detached` 里的实体无法无损建立空间继承，必须清掉 `parent`——否则一旦运行时按 `parent`
/// 做空间继承，它们的世界位置会变，破坏"迁移无损"。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpatialMigration {
    pub locals: BTreeMap<SceneEntityId, String>,
    pub detached: Vec<SceneEntityId>,
}

/// 读取实体的局部变换（2D 优先，其次 3D）。两种都没有时返回 `None`。
pub(crate) fn scene_entity_local(
    entity: &crate::core::scene::SceneEntity,
) -> Option<(SpatialKind, LocalTransform)> {
    if let Some(raw) = entity.components.get("Transform")
        && let Ok(transform) = Transform::from_component_str(raw)
        && let Ok(local) = LocalTransform::from_2d(transform)
    {
        return Some((SpatialKind::TwoD, local));
    }
    if let Some(raw) = entity.components.get("Transform3D")
        && let Ok(transform) = Transform3D::from_component_str(raw)
    {
        return Some((SpatialKind::ThreeD, LocalTransform::from_3d(transform)));
    }
    None
}

/// 把世界矩阵编成该空间的组件串；矩阵不可分解时返回 `None`（调用方按跳过/拒绝处理）。
fn encode_local(kind: SpatialKind, matrix: Mat4) -> Option<String> {
    let local = LocalTransform::from_matrix(matrix).ok()?;
    encode_local_checked(kind, local)
}

/// 把已分解的局部变换编成该空间的组件串；2D 空间要求平面（z 位移/缩放为 1、旋转只绕 Z）。
fn encode_local_checked(kind: SpatialKind, local: LocalTransform) -> Option<String> {
    match kind {
        SpatialKind::ThreeD => Some(
            Transform3D {
                position: local.translation,
                rotation: local.rotation,
                scale: local.scale,
            }
            .to_component_str(),
        ),
        SpatialKind::TwoD => {
            let flat = plane_local(local)?;
            Some(flat.to_component_str())
        }
    }
}

/// 3D 局部变换 → 2D `Transform`：只有真正落在 XY 平面上时才成立（z 位移为 0、z 缩放为 1、
/// 旋转只绕 Z）。返回 `None` 表示"压不平"，调用方按拒绝处理，不做近似。
pub fn plane_local(local: LocalTransform) -> Option<Transform> {
    let flat = crate::core::transform::Transform3D {
        position: local.translation,
        rotation: local.rotation,
        scale: local.scale,
    };
    flat.to_2d()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translated(x: f32) -> LocalTransform {
        LocalTransform {
            translation: Vec3::new(x, 0.0, 0.0),
            ..LocalTransform::identity()
        }
    }

    fn chain(depth: usize) -> TransformTree {
        let mut tree = TransformTree::new();
        for index in 0..depth {
            let id = SceneEntityId(index as u64 + 1);
            tree.set_local(id, translated(1.0));
            if index > 0 {
                tree.set_parent(id, Some(SceneEntityId(index as u64)))
                    .unwrap();
            }
        }
        tree
    }

    #[test]
    fn propagates_depth_100_parent_first() {
        let mut locals = BTreeMap::new();
        let mut parents = BTreeMap::new();
        for index in 0..100_u64 {
            let id = SceneEntityId(index + 1);
            locals.insert(id, translated(1.0));
            if index > 0 {
                parents.insert(id, SceneEntityId(index));
            }
        }
        let globals = propagate_all(&locals, &parents).unwrap();
        let point = globals[&SceneEntityId(100)]
            .matrix()
            .transform_point(Vec3::ZERO)
            .unwrap();
        assert!((point.x - 100.0).abs() < 1e-4);
    }

    #[test]
    fn detects_cycles_and_missing_parents() {
        let mut locals = BTreeMap::new();
        locals.insert(SceneEntityId(1), LocalTransform::identity());
        locals.insert(SceneEntityId(2), LocalTransform::identity());
        let cycle = BTreeMap::from([
            (SceneEntityId(1), SceneEntityId(2)),
            (SceneEntityId(2), SceneEntityId(1)),
        ]);
        assert!(matches!(
            propagate_all(&locals, &cycle),
            Err(PropagationError::Cycle(_))
        ));
        let missing = BTreeMap::from([(SceneEntityId(1), SceneEntityId(9))]);
        assert!(matches!(
            propagate_all(&locals, &missing),
            Err(PropagationError::MissingParent { .. })
        ));
    }

    /// docs/54 §8-②：父先序遍历的**唯一驱动**。这一组把它的语义钉死——运行时传播与加载期
    /// 求解都建立在它之上，因此"什么算坏数据、坏在哪里"只有这一处定义。
    #[test]
    fn parent_first_plan_orders_parents_before_children() {
        // 3 → 2 → 1（父在前）。
        let parents = BTreeMap::from([
            (SceneEntityId(3), SceneEntityId(2)),
            (SceneEntityId(2), SceneEntityId(1)),
        ]);
        let plan = parent_first_plan(
            [SceneEntityId(1), SceneEntityId(2), SceneEntityId(3)],
            |id| parents.get(&id).copied(),
            |_| true,
        );
        assert!(plan.problems.is_empty(), "{plan:?}");
        let position = |id: SceneEntityId| {
            plan.ordered
                .iter()
                .position(|candidate| *candidate == id)
                .expect("ordered")
        };
        assert!(position(SceneEntityId(1)) < position(SceneEntityId(2)));
        assert!(position(SceneEntityId(2)) < position(SceneEntityId(3)));
        assert_eq!(plan.ordered.len(), 3);
    }

    /// 成环：环上成员逐条报出，**后代**报 `UnresolvableAncestor`（不是 `Cycle`——那样会让作者
    /// 去找一个不存在的环），而且结果与实体 id 的大小顺序**无关**（旧实现是遍历顺序相关的）。
    #[test]
    fn parent_first_plan_blocks_the_whole_chain_through_a_cycle_deterministically() {
        // 环：1 ↔ 2；后代：3 → 1；另一个后代 4 → 3。
        let plan_for = |ids: [SceneEntityId; 4]| {
            let parents = BTreeMap::from([
                (SceneEntityId(1), SceneEntityId(2)),
                (SceneEntityId(2), SceneEntityId(1)),
                (SceneEntityId(3), SceneEntityId(1)),
                (SceneEntityId(4), SceneEntityId(3)),
            ]);
            parent_first_plan(ids, |id| parents.get(&id).copied(), |_| true)
        };
        let forward = plan_for([
            SceneEntityId(1),
            SceneEntityId(2),
            SceneEntityId(3),
            SceneEntityId(4),
        ]);
        // 后代 id 更小（会先被访问到）也必须得到同样的结论。
        let reversed = plan_for([
            SceneEntityId(4),
            SceneEntityId(3),
            SceneEntityId(2),
            SceneEntityId(1),
        ]);
        for plan in [&forward, &reversed] {
            assert!(
                plan.ordered.is_empty(),
                "环上成员与其后代都不参与求解：{plan:?}"
            );
            let cycles: Vec<&Vec<SceneEntityId>> = plan
                .problems
                .iter()
                .filter_map(|problem| match problem {
                    PropagationProblem::Cycle(members) => Some(members),
                    _ => None,
                })
                .collect();
            assert_eq!(cycles.len(), 1, "一条环只报一次：{plan:?}");
            let mut members = cycles[0].clone();
            members.sort();
            assert_eq!(members, vec![SceneEntityId(1), SceneEntityId(2)]);
            let blocked: Vec<SceneEntityId> = plan
                .problems
                .iter()
                .filter_map(|problem| match problem {
                    PropagationProblem::UnresolvableAncestor { entity, .. } => Some(*entity),
                    _ => None,
                })
                .collect();
            assert_eq!(
                blocked,
                vec![SceneEntityId(3), SceneEntityId(4)],
                "后代（含孙子）逐个报出：{plan:?}"
            );
        }
    }

    /// 父没有局部变换：本实体与后代都不参与求解，原因是 `ParentWithoutLocal`（旧实现报成环）。
    #[test]
    fn parent_first_plan_reports_a_transformless_parent_instead_of_a_cycle() {
        // 2 没有局部变换；3 是 2 的子；4 是 3 的子。
        let parents = BTreeMap::from([
            (SceneEntityId(3), SceneEntityId(2)),
            (SceneEntityId(4), SceneEntityId(3)),
        ]);
        let plan = parent_first_plan(
            [SceneEntityId(2), SceneEntityId(3), SceneEntityId(4)],
            |id| parents.get(&id).copied(),
            |id| id != SceneEntityId(2),
        );
        assert_eq!(
            plan.ordered,
            vec![SceneEntityId(2)],
            "2 自己没有父，可以求解"
        );
        assert_eq!(
            plan.problems,
            vec![
                PropagationProblem::ParentWithoutLocal {
                    entity: SceneEntityId(3),
                    parent: SceneEntityId(2),
                },
                PropagationProblem::UnresolvableAncestor {
                    entity: SceneEntityId(4),
                    ancestor: SceneEntityId(3),
                },
            ],
            "原因与祖先都要点名"
        );
    }

    /// 缺父（断链）：**仍然求解**（按链根），只是把断链点名——静默当成"没有父"会让子树位移
    /// 看起来毫无原因。
    #[test]
    fn parent_first_plan_keeps_a_dangling_entity_as_a_chain_root_but_reports_it() {
        let parents = BTreeMap::from([(SceneEntityId(1), SceneEntityId(9))]);
        let plan = parent_first_plan(
            [SceneEntityId(1), SceneEntityId(2)],
            |id| parents.get(&id).copied(),
            |_| true,
        );
        assert_eq!(plan.ordered, vec![SceneEntityId(1), SceneEntityId(2)]);
        assert_eq!(
            plan.problems,
            vec![PropagationProblem::MissingParent {
                entity: SceneEntityId(1),
                parent: SceneEntityId(9),
            }]
        );
    }

    #[test]
    fn converts_2d_into_xy_z_rotation_without_losing_scale() {
        let local = LocalTransform::from_2d(Transform {
            position: crate::core::transform::Vec2::new(3.0, 4.0),
            rotation: std::f32::consts::FRAC_PI_2,
            scale: crate::core::transform::Vec2::new(2.0, 5.0),
        })
        .unwrap();
        assert_eq!(local.translation, Vec3::new(3.0, 4.0, 0.0));
        assert_eq!(local.scale, Vec3::new(2.0, 5.0, 1.0));
        let rotated = local.rotation.rotate(Vec3::X).unwrap();
        assert!(rotated.x.abs() < 1e-5 && (rotated.y - 1.0).abs() < 1e-5);
    }

    #[test]
    fn ladder_depths_reach_expected_world_positions() {
        for depth in [1_usize, 10, 100] {
            let mut tree = chain(depth);
            let tip = SceneEntityId(depth as u64);
            let position = tree.world_position(tip).unwrap();
            assert!(
                (position.x - depth as f32).abs() < 1e-3,
                "depth {depth}: {}",
                position.x
            );
        }
    }

    #[test]
    fn incremental_matches_full_propagation() {
        let mut tree = chain(10);
        // 只改动中段一个节点，其余保持缓存。
        tree.set_local(
            SceneEntityId(5),
            LocalTransform {
                translation: Vec3::new(7.0, 1.0, 0.0),
                ..LocalTransform::identity()
            },
        );
        let locals = (1..=10)
            .map(|index| {
                (
                    SceneEntityId(index),
                    tree.local(SceneEntityId(index)).unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let parents = (2..=10)
            .map(|index| (SceneEntityId(index), SceneEntityId(index - 1)))
            .collect::<BTreeMap<_, _>>();
        let reference = propagate_all(&locals, &parents).unwrap();
        for index in 1..=10 {
            let id = SceneEntityId(index);
            let incremental = tree.global(id).unwrap();
            assert_eq!(incremental, reference[&id], "entity {} 不一致", id.0);
        }
    }

    #[test]
    fn incremental_recomputes_only_the_dirty_subtree() {
        let mut tree = chain(100);
        tree.recompute_all().unwrap();
        assert_eq!(tree.recomputes(), 100);
        tree.reset_recomputes();
        // 末端节点变动：只应重算它自己。
        tree.set_local(SceneEntityId(100), translated(2.0));
        tree.global(SceneEntityId(100)).unwrap();
        assert_eq!(tree.recomputes(), 1);
    }

    #[test]
    fn reparent_keep_world_preserves_world_position() {
        let mut tree = TransformTree::new();
        tree.set_local(
            SceneEntityId(1),
            LocalTransform {
                translation: Vec3::new(10.0, 0.0, 0.0),
                scale: Vec3::new(2.0, 2.0, 2.0),
                ..LocalTransform::identity()
            },
        );
        tree.set_local(
            SceneEntityId(2),
            LocalTransform {
                translation: Vec3::new(3.0, 0.0, 0.0),
                ..LocalTransform::identity()
            },
        );
        tree.set_parent(SceneEntityId(2), Some(SceneEntityId(1)))
            .unwrap();
        let before = tree.world_position(SceneEntityId(2)).unwrap();
        assert!((before.x - 16.0).abs() < 1e-4);

        let local = tree.reparent_keep_world(SceneEntityId(2), None).unwrap();
        let after = tree.world_position(SceneEntityId(2)).unwrap();
        assert!((after.x - before.x).abs() < 1e-4, "{after:?}");
        assert!((local.scale.x - 2.0).abs() < 1e-4, "父缩放应烘进局部缩放");

        // 挂回去同样保持世界位置。
        tree.reparent_keep_world(SceneEntityId(2), Some(SceneEntityId(1)))
            .unwrap();
        let back = tree.world_position(SceneEntityId(2)).unwrap();
        assert!((back.x - before.x).abs() < 1e-4, "{back:?}");
    }

    #[test]
    fn rejects_shear_from_nonuniform_parent_scale() {
        let mut tree = TransformTree::new();
        tree.set_local(
            SceneEntityId(1),
            LocalTransform {
                scale: Vec3::new(2.0, 1.0, 1.0),
                ..LocalTransform::identity()
            },
        );
        tree.set_local(
            SceneEntityId(2),
            LocalTransform {
                rotation: Quat::from_axis_angle(Vec3::Z, std::f32::consts::FRAC_PI_4).unwrap(),
                ..LocalTransform::identity()
            },
        );
        tree.set_parent(SceneEntityId(2), Some(SceneEntityId(1)))
            .unwrap();
        // 2:1 父缩放 + 45° 子旋转 = 剪切，无法无损表达为 TRS。
        let error = tree
            .reparent_keep_world(SceneEntityId(2), None)
            .unwrap_err();
        assert!(
            matches!(
                error,
                ReparentError::Decompose(DecomposeError::Shear { .. })
            ),
            "{error:?}"
        );
        // 拒绝时树必须保持原样。
        assert_eq!(
            tree.local(SceneEntityId(2)).unwrap().translation,
            Vec3::ZERO
        );
    }

    #[test]
    fn reparent_rejects_cycles_without_mutating() {
        let mut tree = chain(3);
        let error = tree
            .reparent_keep_world(SceneEntityId(1), Some(SceneEntityId(3)))
            .unwrap_err();
        assert_eq!(error, ReparentError::WouldCycle(SceneEntityId(3)));
        assert!(tree.local(SceneEntityId(1)).is_some());
    }

    #[test]
    fn round_trips_trs_through_matrix() {
        let local = LocalTransform {
            translation: Vec3::new(1.5, -2.0, 3.25),
            rotation: Quat::from_axis_angle(Vec3::Y, 0.7).unwrap(),
            scale: Vec3::new(2.0, 0.5, 1.25),
        };
        let restored = LocalTransform::from_matrix(local.matrix().unwrap()).unwrap();
        assert!((restored.translation.x - local.translation.x).abs() < 1e-5);
        assert!((restored.translation.y - local.translation.y).abs() < 1e-5);
        assert!((restored.scale.x - local.scale.x).abs() < 1e-4);
        assert!((restored.scale.z - local.scale.z).abs() < 1e-4);
        let point = Vec3::new(1.0, 2.0, -3.0);
        let expected = local.matrix().unwrap().transform_point(point).unwrap();
        let actual = restored.matrix().unwrap().transform_point(point).unwrap();
        assert!((expected.x - actual.x).abs() < 1e-4);
        assert!((expected.y - actual.y).abs() < 1e-4);
        assert!((expected.z - actual.z).abs() < 1e-4);
    }

    #[test]
    fn rejects_mirrored_and_degenerate_matrices() {
        let mirrored = Mat4::scale(Vec3::new(-1.0, 1.0, 1.0));
        assert_eq!(
            LocalTransform::from_matrix(mirrored),
            Err(DecomposeError::Mirrored)
        );
        let degenerate = Mat4::scale(Vec3::new(0.0, 1.0, 1.0));
        assert_eq!(
            LocalTransform::from_matrix(degenerate),
            Err(DecomposeError::DegenerateScale)
        );
    }

    /// 旧新对拍：项目里每个真实 `Transform3D` 经统一表示产出的矩阵，必须与旧
    /// `Transform3D::model_matrix()` 逐元素一致——统一空间不得改变既有 3D 画面。
    #[test]
    fn unified_matrix_matches_legacy_model_matrix_on_project_scenes() {
        use crate::core::scene::Scene;
        let roots = [
            crate::core::games_dir().join("examples/game_api/scenes"),
            crate::core::games_dir().join("examples/game_jhyt/scenes"),
        ];
        let mut checked = 0;
        for root in roots {
            let entries = std::fs::read_dir(&root).expect("场景目录");
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("scene") {
                    continue;
                }
                let scene = Scene::load(&path).expect("load scene");
                for entity in scene.entities.values() {
                    let Some(raw) = entity.components.get("Transform3D") else {
                        continue;
                    };
                    let legacy = Transform3D::from_component_str(raw).expect("parse Transform3D");
                    let legacy_matrix = legacy.model_matrix().expect("legacy model matrix");
                    let unified_matrix = LocalTransform::from_3d(legacy)
                        .matrix()
                        .expect("unified matrix");
                    for column in 0..4 {
                        for row in 0..4 {
                            let expected = legacy_matrix.columns[column][row];
                            let actual = unified_matrix.columns[column][row];
                            assert!(
                                (expected - actual).abs() < 1e-6,
                                "{} {} 列{column}行{row}: {expected} vs {actual}",
                                path.display(),
                                entity.name
                            );
                        }
                    }
                    checked += 1;
                }
            }
        }
        assert!(
            checked >= 3,
            "应至少覆盖 3D 场景的 Transform3D，实际 {checked}"
        );
    }

    fn scene_with_transform3d(name: &str, entries: &[(u64, Option<u64>, &str)]) -> Scene {
        let mut scene = Scene::new(name);
        for (id, parent, transform) in entries {
            scene
                .add_entity(
                    SceneEntityId(*id),
                    format!("E{id}"),
                    parent.map(SceneEntityId),
                )
                .expect("add entity");
            scene
                .entities
                .get_mut(&SceneEntityId(*id))
                .expect("entity")
                .components
                .insert("Transform3D".into(), (*transform).to_owned());
        }
        scene
    }

    /// 迁移必须**保持世界位置**，而不是把旧局部值当成相对父节点的坐标。
    /// 后者会把子实体从 x=3 搬到 x=16——正是 docs/40 §3 WP2 禁止的语义漂移。
    #[test]
    fn migration_keeps_world_positions_instead_of_treating_parent_as_local() {
        let scene = scene_with_transform3d(
            "migrate",
            &[
                (1, None, "position=10,0,0;rotation=0,0,0,1;scale=2,2,2"),
                (2, Some(1), "position=3,0,0;rotation=0,0,0,1;scale=1,1,1"),
            ],
        );
        let (mut tree, report) = migrate_v2_hierarchy(&scene);
        assert!(report.is_lossless(), "{report:?}");
        assert_eq!(report.converted, vec![SceneEntityId(2)]);
        assert_eq!(report.roots, vec![SceneEntityId(1)]);

        let position = tree.world_position(SceneEntityId(2)).unwrap();
        assert!((position.x - 3.0).abs() < 1e-4, "{position:?}");
        assert!(
            (position.x - 16.0).abs() > 1.0,
            "不得把旧局部值当作相对父节点的坐标"
        );
        // 父缩放被烘进子局部缩放。
        let local = tree.local(SceneEntityId(2)).unwrap();
        assert!((local.scale.x - 0.5).abs() < 1e-4, "{local:?}");
        assert!((local.translation.x + 3.5).abs() < 1e-4, "{local:?}");
    }

    /// 无法无损表达时拒绝迁移，并保持实体为根——世界位置同样不变。
    #[test]
    fn migration_refuses_shear_and_keeps_world_position() {
        let scene = scene_with_transform3d(
            "shear",
            &[
                (1, None, "position=0,0,0;rotation=0,0,0,1;scale=2,1,1"),
                (
                    2,
                    Some(1),
                    "position=5,0,0;rotation=0,0,0.3826834,0.9238795;scale=1,1,1",
                ),
            ],
        );
        let (mut tree, report) = migrate_v2_hierarchy(&scene);
        assert!(!report.is_lossless());
        assert_eq!(report.refused.len(), 1);
        assert_eq!(report.refused[0].0, SceneEntityId(2));
        assert!(matches!(
            report.refused[0].1,
            MigrationRefusal::NotRepresentable(DecomposeError::Shear { .. })
        ));
        let position = tree.world_position(SceneEntityId(2)).unwrap();
        assert!((position.x - 5.0).abs() < 1e-4, "{position:?}");
    }

    /// 真实工程场景：迁移后每个实体的世界位置必须与它原来的 TRS 位置一致。
    #[test]
    fn migration_preserves_world_positions_on_project_scenes() {
        let roots = [
            crate::core::games_dir().join("examples/game_api/scenes"),
            crate::core::games_dir().join("examples/game_jhyt/scenes"),
        ];
        let mut checked = 0;
        for root in roots {
            let entries = std::fs::read_dir(&root).expect("场景目录");
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("scene") {
                    continue;
                }
                let scene = Scene::load(&path).expect("load scene");
                let (mut tree, report) = migrate_v2_hierarchy(&scene);
                assert!(
                    report.is_lossless(),
                    "{} 迁移被拒绝：{:?}",
                    path.display(),
                    report.refused
                );
                for entity in scene.entities.values() {
                    let Some(original) = entity
                        .components
                        .get("Transform3D")
                        .and_then(|raw| Transform3D::from_component_str(raw).ok())
                    else {
                        continue;
                    };
                    let migrated = tree.world_position(entity.id).unwrap();
                    assert!(
                        (migrated.x - original.position.x).abs() < 1e-3
                            && (migrated.y - original.position.y).abs() < 1e-3
                            && (migrated.z - original.position.z).abs() < 1e-3,
                        "{} {} 世界位置漂移：{:?} → {:?}",
                        path.display(),
                        entity.name,
                        original.position,
                        migrated
                    );
                    checked += 1;
                }
            }
        }
        assert!(
            checked >= 3,
            "应至少覆盖 3D 场景的 Transform3D，实际 {checked}"
        );
    }

    /// 把迁移写回的组件串解析回局部变换（2D 优先，其次 3D）。
    fn local_from_encoded(raw: &str) -> LocalTransform {
        if let Ok(transform) = Transform::from_component_str(raw) {
            return LocalTransform::from_2d(transform).expect("2D local is valid");
        }
        LocalTransform::from_3d(Transform3D::from_component_str(raw).expect("3D local parses"))
    }

    fn scene_with_2d(name: &str, entries: &[(u64, Option<u64>, &str)]) -> Scene {
        let mut scene = Scene::new(name);
        for (id, parent, transform) in entries {
            scene
                .add_entity(
                    SceneEntityId(*id),
                    format!("E{id}"),
                    parent.map(SceneEntityId),
                )
                .expect("add entity");
            scene
                .entities
                .get_mut(&SceneEntityId(*id))
                .expect("entity")
                .components
                .insert("Transform".into(), (*transform).to_owned());
        }
        scene
    }

    /// WP2 裁定要求"所有场景 parent 按局部空间解释"，而真实场景里 **2D 实体占多数**——
    /// 因此迁移必须同时覆盖 2D，并且同样**保持世界位置**。
    #[test]
    fn two_d_migration_keeps_world_positions() {
        let scene = scene_with_2d(
            "migrate-2d",
            &[
                (1, None, "x=10;y=5;rotation=0;scale_x=2;scale_y=2"),
                // 子实体的 x=3 是**世界**坐标（v2 语义），迁移后局部 x 应为 (3-10)/2 = -3.5。
                (2, Some(1), "x=3;y=5;rotation=0;scale_x=1;scale_y=1"),
            ],
        );
        let (migrated, report) = migrate_scene_to_local_space(&scene);
        assert!(report.is_lossless(), "{report:?}");
        assert_eq!(report.converted, vec![SceneEntityId(2)]);
        let child = Transform::from_component_str(
            migrated
                .locals
                .get(&SceneEntityId(2))
                .expect("child encoded"),
        )
        .expect("child parses");
        assert!(
            (child.position.x - (-3.5)).abs() < 1e-4,
            "局部 x 应为 (3-10)/2，实际 {}",
            child.position.x
        );
        assert!(
            migrated
                .locals
                .get(&SceneEntityId(1))
                .expect("root encoded")
                .contains("x=10.000000000"),
            "根实体也写回规范串"
        );
        assert!(migrated.detached.is_empty());

        // 世界位置真的没变：把迁移结果当作**局部**值做父先序传播，世界位置必须与原来一致。
        // （不能再用 `migrate_v2_hierarchy` 验证——那个函数总会把输入当作 v2 的"局部=世界"
        // 再相对父节点重算一次，喂给它已迁移的数据等于二次迁移。）
        let mut locals = BTreeMap::new();
        for (id, raw) in &migrated.locals {
            locals.insert(*id, local_from_encoded(raw));
        }
        let mut parents = BTreeMap::new();
        parents.insert(SceneEntityId(2), SceneEntityId(1));
        let globals = propagate_all(&locals, &parents).expect("propagate migrated locals");
        let world = Vec3::new(
            globals[&SceneEntityId(2)].matrix().columns[3][0],
            globals[&SceneEntityId(2)].matrix().columns[3][1],
            globals[&SceneEntityId(2)].matrix().columns[3][2],
        );
        assert!(
            (world.x - 3.0).abs() < 1e-3 && (world.y - 5.0).abs() < 1e-3,
            "迁移后世界位置漂移：{world:?}"
        );
    }

    /// 2D 里"压不平"的父子关系（父有旋转 + 非均匀缩放 ⇒ 子世界矩阵含剪切）必须**拒绝迁移、
    /// 清掉 `parent` 并保持世界位置**，而不是近似成一个会改变画面的局部变换。
    #[test]
    fn two_d_migration_refuses_sheared_children_and_keeps_them_as_roots() {
        let scene = scene_with_2d(
            "migrate-2d-shear",
            &[
                (1, None, "x=0;y=0;rotation=45;scale_x=2;scale_y=1"),
                (2, Some(1), "x=0;y=0;rotation=-45;scale_x=1;scale_y=1"),
            ],
        );
        let (migrated, report) = migrate_scene_to_local_space(&scene);
        assert!(!report.is_lossless(), "含剪切的父子关系必须被拒绝");
        assert_eq!(migrated.detached, vec![SceneEntityId(2)]);
        // 被拒实体保持为根 ⇒ 世界位置仍是它自己的 TRS，迁移前后一致。
        let kept =
            Transform::from_component_str(migrated.locals.get(&SceneEntityId(2)).expect("encoded"))
                .expect("parses");
        assert!(kept.position.x.abs() < 1e-6 && kept.position.y.abs() < 1e-6);
    }
}
