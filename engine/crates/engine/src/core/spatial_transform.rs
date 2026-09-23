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

/// **唯一的变换组件名**：场景/Prefab 里 2D 与 3D 实体写的都是这一个名字。
pub const TRANSFORM_COMPONENT: &str = "Transform";

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

    pub fn from_3d(value: Transform3D) -> Self {
        Self {
            translation: value.position,
            rotation: value.rotation,
            scale: value.scale,
        }
    }

    /// **统一 `Transform` 的 2D 视图**：位置取 xy、旋转取绕 Z 的角度、缩放取 xy。
    ///
    /// 给 2D 消费者的唯一入口（2D 渲染、2D 拖拽、2D 物理都用它）：统一后的组件是 3D 表示，
    /// 而 2D 语义（`x/y/rotation/scale_x/scale_y`）由这里投影出来，**2D 代码不必知道 3D 形态**。
    /// 旋转按 `2*atan2(z, w)` 取 Z 分量：对"绕 Z 旋转"的载荷是精确的，对一般四元数是投影。
    #[must_use]
    pub fn to_2d(self) -> Transform {
        Transform {
            position: crate::core::transform::Vec2::new(self.translation.x, self.translation.y),
            rotation: 2.0 * self.rotation.z.atan2(self.rotation.w),
            scale: crate::core::transform::Vec2::new(self.scale.x, self.scale.y),
        }
    }

    /// 读 `Transform` 组件并给出 **2D 视图**（2D 便利 API 的唯一入口）；畸形串返回 `None`。
    #[must_use]
    pub fn transform_2d_from_component_str(raw: &str) -> Option<Transform> {
        Self::from_unified_component_str(raw).ok().map(Self::to_2d)
    }

    /// `Transform` 组件的读取器：**唯一形态**是 `position=x,y,z;rotation=rx,ry,rz,rw;scale=sx,sy,sz`
    /// （直接委托 [`Transform3D::from_component_str`]，数字与报错文案只有那一份实现）。
    ///
    /// 2D 与 3D 实体用的是同一个组件、同一个形态：空间差别由实体携带的其它组件推断
    /// （见 [`entity_space`]），2D 消费者用 [`Self::to_2d`] 把统一形态投影成平面 TRS。
    pub fn from_unified_component_str(value: &str) -> Result<Self, String> {
        Transform3D::from_component_str(value)
            .map(Self::from_3d)
            .map_err(|error| error.to_string())
    }

    /// 统一 `Transform` 组件的**规范写法**（3D 形态）。
    ///
    /// 委托 [`Transform3D::to_component_str`]，数字格式只有那一份实现。
    #[must_use]
    pub fn to_unified_component_str(self) -> String {
        Transform3D {
            position: self.translation,
            rotation: self.rotation,
            scale: self.scale,
        }
        .to_component_str()
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
    // 排序 `Vec` + 平行 `state`，而不是 `BTreeSet`/`BTreeMap`：后者的**每个实体一次树节点分配**
    // 在运行时传播里是热点（docs/53 §3.4c 实测：1 万实体的层级帧每帧 4.5 万次分配、传播占一帧
    // 三成）。排序 + 二分把每实体的分配降到 0，且顺序与原来的 `BTreeSet`（按 id 升序）**完全一致**，
    // 因此 `ordered` 与 `problems` 的确定性不变。
    let mut ids: Vec<Id> = ids.into_iter().collect();
    ids.sort_unstable();
    ids.dedup();
    let mut ordered = Vec::with_capacity(ids.len());
    let mut problems = Vec::new();
    // 0 = 未访问，1 = 当前链上，2 = 已求解，3 = 不可解。下标与 `ids` 一一对应。
    let mut state: Vec<u8> = vec![0; ids.len()];
    let index_of = |ids: &[Id], id: Id| ids.binary_search(&id).ok();
    // 链上暂存**只分配一次**：此前每条链一个 `Vec`（1,250 条链就是每帧 1,250 次分配）。
    let mut chain: Vec<Id> = Vec::new();
    for start_index in 0..ids.len() {
        let start = ids[start_index];
        if state[start_index] != 0 {
            continue;
        }
        // 沿父链上溯；`chain` 从 start 一路指向上游。
        chain.clear();
        let mut cursor = Some((start_index, start));
        // 本次链里第一个"解不了"的实体（环成员或父无局部变换者），以及它的原因。
        let mut unresolved_from: Option<(usize, Id)> = None;
        while let Some((entity_index, entity)) = cursor {
            match state[entity_index] {
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
                        if let Some(index) = index_of(&ids, *member) {
                            state[index] = 3;
                        }
                    }
                    problems.push(PropagationProblem::Cycle(members.clone()));
                    // 环**上方**的那一段（更靠近 start）是环的后代：它们同样不参与求解。
                    chain.truncate(position);
                    unresolved_from = Some((chain.len(), members[0]));
                    break;
                }
                _ => {}
            }
            state[entity_index] = 1;
            // 链根判定：没有父，或父不在集合里（后一种要报告，但仍按链根求解）。
            let Some(parent) = parent_of(entity) else {
                chain.push(entity);
                break;
            };
            let Some(parent_index) = index_of(&ids, parent) else {
                problems.push(PropagationProblem::MissingParent { entity, parent });
                chain.push(entity);
                break;
            };
            if !has_local(parent) {
                problems.push(PropagationProblem::ParentWithoutLocal { entity, parent });
                chain.push(entity);
                unresolved_from = Some((chain.len() - 1, entity));
                break;
            }
            chain.push(entity);
            cursor = Some((parent_index, parent));
        }
        match unresolved_from {
            Some((from, ancestor)) => {
                for (index, entity) in chain.iter().enumerate() {
                    if index_of(&ids, *entity).is_some_and(|slot| state[slot] == 3) {
                        continue; // 环成员已经报过
                    }
                    if let Some(slot) = index_of(&ids, *entity) {
                        state[slot] = 3;
                    }
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
                    if let Some(slot) = index_of(&ids, *entity)
                        && state[slot] == 1
                    {
                        state[slot] = 2;
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
        let Some(local) = scene_entity_local(entity) else {
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
    let mut locals: BTreeMap<SceneEntityId, LocalTransform> = BTreeMap::new();
    for entity in scene.entities.values() {
        match scene_entity_local(entity) {
            Some(local) => match local.matrix() {
                Ok(matrix) => {
                    world.insert(entity.id, matrix);
                    locals.insert(entity.id, local);
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
        // 保持世界位置的两种结果：建继承（局部 = 父世界的逆 × 自己世界）或保持为根。
        // 保持为根时局部值就是原值（v2 的"局部=世界"），因此直接回写、不重新分解。
        let own_local = locals[&entity.id];
        let keep_as_root = |migrated: &mut SpatialMigration| {
            migrated.locals.insert(entity.id, encode_local(own_local));
        };
        let Some(parent) = entity.parent else {
            keep_as_root(&mut migrated);
            report.roots.push(entity.id);
            continue;
        };
        let Some(&parent_world) = world.get(&parent) else {
            // 父节点没有可用的变换：保持组织关系但不建立空间继承，世界位置不变。
            keep_as_root(&mut migrated);
            report.skipped.push(entity.id);
            continue;
        };
        match GlobalTransform::from_matrix(own_world)
            .local_relative_to(Some(GlobalTransform::from_matrix(parent_world)))
        {
            Ok(local) => {
                migrated.locals.insert(entity.id, encode_local(local));
                report.converted.push(entity.id);
            }
            Err(error) => {
                // 拒绝：保持为根，并**清掉 parent**，否则新语义下世界位置会变。
                keep_as_root(&mut migrated);
                report
                    .refused
                    .push((entity.id, MigrationRefusal::NotRepresentable(error)));
                migrated.detached.push(entity.id);
            }
        }
    }
    (migrated, report)
}

/// 实体属于哪种空间（统一 `Transform` 之后由实体携带的其它组件推断，见 [`entity_space`]）。
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

/// 读取实体的局部变换（统一 `Transform`）。没有组件或载荷非法时返回 `None`。
pub(crate) fn scene_entity_local(
    entity: &crate::core::scene::SceneEntity,
) -> Option<LocalTransform> {
    LocalTransform::from_unified_component_str(entity.components.get(TRANSFORM_COMPONENT)?).ok()
}

/// **3D 专属组件**：实体带其中任意一个，就判定它属于 3D 空间（见 [`entity_space`]）。
///
/// 判据必须**数据可验证**，不能是"看起来对"的猜测：仓库内两个示例工程在去掉旧名之前，
/// 每个带 `Transform3D` 的实体都恰好带这里列出的某一个（一次性迁移工具在改名之前逐实体断言过，
/// 见 docs/56 §3.52），所以"旧 3D 名"与"带 3D 专属组件"在真实数据上是同一个划分。
pub const THREE_D_MARKERS: &[&str] = &["MeshRenderer", "Animation3D", "Collider3d", "Light"];

/// 实体的**空间种类**：统一 `Transform` 之后磁盘上不再有"2D 名 / 3D 名"可依据，
/// 所以由实体携带的**其它组件**推断。
///
/// 规则（顺序固定，全部写在 [`THREE_D_MARKERS`] 与这里）：
/// 1. 带任一 3D 专属组件 ⇒ [`SpatialKind::ThreeD`]；
/// 2. 带**透视**相机（统一 `Camera` 的 `projection=perspective`）⇒ [`SpatialKind::ThreeD`]；
/// 3. 其余一律 [`SpatialKind::TwoD`]——2D 是历史默认（`Transform` 从来就是 2D 的名字）。
///
/// **已知边界**（诚实记录）：一个"既没有 3D 专属组件、又要 3D 变换"的实体会落到 2D 空间。
/// 现有示例工程里不存在这种实体（3D 画面全都由 `MeshRenderer`/`Light`/`Animation3D` 产生）。
/// 反向的误判不会发生：2D 实体带上 `Light` 之类的组件在本引擎里本身就是矛盾数据。
#[must_use]
pub fn entity_space(components: &BTreeMap<String, String>) -> SpatialKind {
    if THREE_D_MARKERS
        .iter()
        .any(|name| components.contains_key(*name))
    {
        return SpatialKind::ThreeD;
    }
    let perspective = components
        .get(crate::core::camera::CAMERA_COMPONENT)
        .and_then(|raw| crate::core::camera::Camera::from_component_str(raw).ok())
        .is_some_and(|camera| {
            camera.projection() == crate::core::camera::CameraProjection::Perspective
        });
    if perspective {
        SpatialKind::ThreeD
    } else {
        SpatialKind::TwoD
    }
}

/// 把已分解的局部变换编成**唯一的**组件串（统一 3D 形态）。
fn encode_local(local: LocalTransform) -> String {
    local.to_unified_component_str()
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

    /// **空间推断规则**（第 57 轮）：统一 `Transform` 之后磁盘上不再有"2D 名 / 3D 名"，
    /// 2D/3D 由实体携带的**其它组件**推断（[`entity_space`]）。这条把规则表逐条钉住：
    ///
    /// - 任一 3D 专属组件（[`THREE_D_MARKERS`]）⇒ 3D；
    /// - 透视 `Camera` ⇒ 3D；正交 `Camera` ⇒ 2D；
    /// - 其余（含"只有 `Transform`"）⇒ 2D。
    ///
    /// **为什么可信**：仓库内两个示例工程在改名之前，每个 `Transform3D` 实体都恰好带某个
    /// 3D 专属组件（一次性迁移工具在改名之前逐实体断言过，见 docs/56 §3.52），所以
    /// "旧 3D 名"与"带 3D 专属组件"在真实数据上是**同一个划分**。
    #[test]
    fn entity_space_is_inferred_from_the_other_components() {
        let components = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect()
        };
        let transform = "position=1,2,3;rotation=0,0,0,1;scale=1,1,1";
        // 标记表**逐字**断言：删掉/替换其中一条（例如把 `MeshRenderer` 移出）必须判红，
        // 否则"规则表"就只是测试自己引用的一个常量，改它不会有人发现。
        assert_eq!(
            THREE_D_MARKERS,
            &["MeshRenderer", "Animation3D", "Collider3d", "Light"]
        );
        // 只有变换 ⇒ 2D（历史默认）。
        assert_eq!(
            entity_space(&components(&[("Transform", transform)])),
            SpatialKind::TwoD
        );
        // 任一 3D 专属组件 ⇒ 3D（逐个断言，标记表被删空一条就会红）。
        for marker in THREE_D_MARKERS {
            assert_eq!(
                entity_space(&components(&[("Transform", transform), (marker, "x=1")])),
                SpatialKind::ThreeD,
                "{marker} 必须是 3D 专属标记"
            );
        }
        // 2D 专属组件**不**改变判定（默认就是 2D，它们只是佐证）。
        for marker in ["Sprite", "Quad", "Tilemap", "UiLayout", "Text", "Collider"] {
            assert_eq!(
                entity_space(&components(&[("Transform", transform), (marker, "x=1")])),
                SpatialKind::TwoD,
                "{marker} 不该把实体判成 3D"
            );
        }
        // 相机的**投影判别字段**决定空间（不是组件名）。
        assert_eq!(
            entity_space(&components(&[(
                "Camera",
                "projection=perspective;position=0,0,5;target=0,0,0;fov=60;near=0.1;far=100"
            )])),
            SpatialKind::ThreeD
        );
        assert_eq!(
            entity_space(&components(&[(
                "Camera",
                "projection=orthographic;center=0,0;zoom=1"
            )])),
            SpatialKind::TwoD
        );
        // 坏的相机载荷**不**把实体拖进 3D（推断不出来就按默认 2D；载荷本身由各自路径报错）。
        assert_eq!(
            entity_space(&components(&[("Camera", "position=oops")])),
            SpatialKind::TwoD
        );
    }

    /// **统一 `Transform` 的读侧**（第 52 轮）：只有**一种**形态，即
    /// `position=x,y,z;rotation=rx,ry,rz,rw;scale=sx,sy,sz`。
    ///
    /// 收敛前读侧还兼收 2D 旧写法（`x=..;y=..;rotation=..;scale_x=..;scale_y=..`）；旧名与旧形态
    /// 删除之后，那条通路必须**同时**关闭，否则"某个 2D 实体悄悄写旧形态"会重新变得可读。
    #[test]
    fn unified_transform_reader_accepts_only_the_canonical_shape() {
        // 统一写法：位置/四元数/缩放原样读入。
        let three_d = Transform3D {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_axis_angle(Vec3::Y, 0.5).expect("四元数"),
            scale: Vec3::new(2.0, 2.0, 2.0),
        }
        .to_component_str();
        let parsed = LocalTransform::from_unified_component_str(&three_d).expect("统一写法可读");
        assert_eq!(
            parsed,
            LocalTransform::from_3d(Transform3D::from_component_str(&three_d).unwrap())
        );
        assert_eq!(
            parsed.to_unified_component_str(),
            three_d,
            "规范化后逐字节相同"
        );

        // 2D 消费者的唯一入口是 `to_2d` 投影：绕 Z 的旋转还原成平面角度、缩放取 xy。
        let quarter_turn = std::f32::consts::FRAC_PI_2;
        let planar = LocalTransform::from_unified_component_str(
            &Transform3D {
                position: Vec3::new(3.0, -4.0, 0.0),
                rotation: Quat::from_axis_angle(Vec3::Z, quarter_turn).expect("四元数"),
                scale: Vec3::new(2.0, 0.5, 1.0),
            }
            .to_component_str(),
        )
        .expect("统一写法可读");
        let planar_2d = planar.to_2d();
        assert_eq!(
            planar_2d.position,
            crate::core::transform::Vec2::new(3.0, -4.0)
        );
        assert_eq!(planar_2d.scale, crate::core::transform::Vec2::new(2.0, 0.5));
        assert!(
            (planar_2d.rotation - quarter_turn).abs() < 1e-5,
            "{}",
            planar_2d.rotation
        );

        // 读→写→读 稳定（编辑器保存出去的统一写法，再读回来必须一样）。
        let canonical = parsed.to_unified_component_str();
        assert_eq!(
            LocalTransform::from_unified_component_str(&canonical).expect("统一写法可读"),
            parsed
        );
        // 2D 旧写法已随旧名一起删除：读侧必须拒绝它（不再有"两种历史写法"这回事）。
        assert!(
            LocalTransform::from_unified_component_str(
                "x=3.000000000;y=-4.000000000;rotation=1.570796327;scale_x=2.000000000;scale_y=0.500000000"
            )
            .is_err(),
            "2D 旧形态已不存在，读侧必须拒绝"
        );
        // 畸形串一律失败，返回底层解析器的原文，不在这里发明新文案。
        assert!(LocalTransform::from_unified_component_str("x=oops").is_err());
        assert!(
            LocalTransform::from_unified_component_str("position=1,2").is_err(),
            "缺字段的统一串必须被拒"
        );
        assert!(LocalTransform::from_unified_component_str("").is_err());
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

    /// 对拍：项目里每个真实 3D 实体的统一 `Transform` 经统一表示产出的矩阵，必须与
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
                    // 3D 实体 = 承载 3D 专属组件（或透视相机）的实体，见 `entity_space`。
                    if entity_space(&entity.components) != SpatialKind::ThreeD {
                        continue;
                    }
                    let Some(raw) = entity.components.get(TRANSFORM_COMPONENT) else {
                        continue;
                    };
                    let legacy = Transform3D::from_component_str(raw).expect("parse Transform");
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
            "应至少覆盖 3D 场景的 Transform，实际 {checked}"
        );
    }

    fn scene_with_transform(name: &str, entries: &[(u64, Option<u64>, &str)]) -> Scene {
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
                .insert(TRANSFORM_COMPONENT.into(), (*transform).to_owned());
        }
        scene
    }

    /// 迁移必须**保持世界位置**，而不是把旧局部值当成相对父节点的坐标。
    /// 后者会把子实体从 x=3 搬到 x=16——正是 docs/40 §3 WP2 禁止的语义漂移。
    #[test]
    fn migration_keeps_world_positions_instead_of_treating_parent_as_local() {
        let scene = scene_with_transform(
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
        let scene = scene_with_transform(
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
                        .get(TRANSFORM_COMPONENT)
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
            "应至少覆盖 3D 场景的 Transform，实际 {checked}"
        );
    }

    /// 把迁移写回的组件串解析回局部变换（唯一形态 + 2D 投影视图）。
    fn local_from_encoded(raw: &str) -> LocalTransform {
        LocalTransform::from_unified_component_str(raw).expect("unified local parses")
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
                (1, None, "position=10,5,0;rotation=0,0,0,1;scale=2,2,1"),
                // 子实体的 x=3 是**世界**坐标（v2 语义），迁移后局部 x 应为 (3-10)/2 = -3.5。
                (2, Some(1), "position=3,5,0;rotation=0,0,0,1;scale=1,1,1"),
            ],
        );
        let (migrated, report) = migrate_scene_to_local_space(&scene);
        assert!(report.is_lossless(), "{report:?}");
        assert_eq!(report.converted, vec![SceneEntityId(2)]);
        let child = LocalTransform::transform_2d_from_component_str(
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
                .contains("position=10.000000000,5.000000000,0.000000000"),
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
                // `rotation=45` 旧载荷是**弧度**原值 ⇒ 四元数取半角 `sin/cos(22.5)`。
                (
                    1,
                    None,
                    "position=0,0,0;rotation=0,0,-0.48717451246050952,-0.87330464009351561;scale=2,1,1",
                ),
                (
                    2,
                    Some(1),
                    "position=0,0,0;rotation=0,0,0.48717451246050952,-0.87330464009351561;scale=1,1,1",
                ),
            ],
        );
        let (migrated, report) = migrate_scene_to_local_space(&scene);
        assert!(!report.is_lossless(), "含剪切的父子关系必须被拒绝");
        assert_eq!(migrated.detached, vec![SceneEntityId(2)]);
        // 被拒实体保持为根 ⇒ 世界位置仍是它自己的 TRS，迁移前后一致。
        let kept = LocalTransform::transform_2d_from_component_str(
            migrated.locals.get(&SceneEntityId(2)).expect("encoded"),
        )
        .expect("parses");
        assert!(kept.position.x.abs() < 1e-6 && kept.position.y.abs() < 1e-6);
    }
}
