//! 3D **实例批次**（crowd）：一个实体带 N 份实例变换，渲染器一次实例化绘制。
//!
//! 为什么要有它：一窝蚂蚁是 N 个个体，但它们共用一张网格与一份材质。逐个建实体的做法让
//! **ECS 与每帧提取**都按 N 走——每个实体一份 `MeshRenderer`（两个 `String`）、一次组件查找、
//! 一个 drawable。批次把 N 份变换收进**一个**组件，渲染侧仍走同一条已存在的实例化路径
//! （`instance_models` 存储缓冲 + `draw_indexed(..., 0..instances)`），于是"N 万只蚂蚁"
//! 在绘制侧是**一次** draw call，提取侧也只有**一个**实体。
//!
//! 口径（刻意收窄，避免变成"什么都能塞"的口袋）：
//! - **实例坐标是世界坐标**；批次实体自身的 `Transform3D` 不参与（批次是绝对摆位）。
//! - **一张网格、一份材质**：批次内不区分 LOD、不区分网格变体。要这两种就开两个批次。
//! - **不做逐实例可见性**：可见性跟普通网格一样按**批次实体**判（`visibility` 组件）。
//! - **不做逐实例骨骼动画**：姿态写在实例矩阵里（骨骼动画仍走 `Animation3D` 的普通实体）。
//!
//! P3-R238：**程序化运动**。批次可以带一段路径，实例按**弧长**沿路径流动，姿态由**引擎**
//! 每帧求值——脚本只写一次静态表，不再每帧重建 N 份实例变换。动机是实测（Myrmex
//! `docs/02-balance.md` §21.517）：Lua 侧每帧重建实例表要 3.0 µs/只，6 万只就是 ≈180 ms/帧，
//! 而引擎侧提取只要 0.19 µs/只 ⇒ 每帧的 N 份变换必须由引擎算，脚本只负责"写一次"。

use rustlua_math::math3d::{Mat4, Quat, Vec3};

/// 批次级程序化运动的类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionKind {
    /// 沿折线路径流动（弧长参数化，路径可闭合）。
    Path,
}

/// 批次级程序化运动：引擎按场景时钟求值实例姿态。
#[derive(Clone, Debug, PartialEq)]
pub struct BatchMotion3D {
    /// 运动类型（目前只有路径流）。
    pub kind: MotionKind,
    /// 路径点（世界坐标）。至少两个有效点；与前一点重合的退化点会被跳过。
    pub path: Vec<[f32; 3]>,
    /// 路径是否闭合（首尾相接，按弧长循环）。
    pub closed: bool,
    /// 基础速度（世界单位/秒）；负值 = 逆向流动。
    pub speed: f32,
}

/// 实例姿态跟随路径切线时，模型的**前向轴**（哪根轴指向行进方向）。
///
/// 姿态用"绕 +Y 的 yaw"表达（quat = (0, sin(θ/2), 0, cos(θ/2))），而 yaw 落到哪根轴上
/// 取决于模型自己的约定：绕 +Y 转 θ 时模型 +X → 世界 (cos θ, 0, −sin θ)、+Z → (sin θ, 0,
/// cos θ)。于是同一段路径，"+X 前向"与"+Z 前向"两种模型差 90°——引擎不能替模型猜。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ForwardAxis {
    /// 模型 +X 轴指向行进方向（默认，保持既有行为）。
    #[default]
    X,
    /// 模型 +Z 轴指向行进方向（"`yaw` ⇒ (sin yaw, cos yaw) 是前向"的那类游戏约定）。
    Z,
}

/// 逐实例的运动参数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InstanceMotion3D {
    /// 沿路径的起始弧长偏移（世界单位）。
    pub offset: f32,
    /// 速度倍率（乘批次速度）。`0` = 停在路径起点。
    pub speed: f32,
    /// 姿态是否跟随路径切线（`false` = 用实例表里的静态姿态）。
    pub align: bool,
    /// `align` 时模型的前向轴（默认 +X）。
    pub forward: ForwardAxis,
}

impl Default for InstanceMotion3D {
    fn default() -> Self {
        Self {
            offset: 0.0,
            speed: 1.0,
            align: false,
            forward: ForwardAxis::X,
        }
    }
}

/// 单个实例：世界位置 + 四元数姿态 + 缩放（+ 可选的逐实例运动参数）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance3D {
    /// 世界坐标 x/y/z。**批次带运动时本项被忽略**（位置由路径决定）。
    pub position: [f32; 3],
    /// 四元数 x/y/z/w。
    pub rotation: [f32; 4],
    /// 缩放 x/y/z。
    pub scale: [f32; 3],
    /// 逐实例运动参数（批次没有运动时本项无效果）。
    pub motion: Option<InstanceMotion3D>,
}

impl Default for Instance3D {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            motion: None,
        }
    }
}

impl Instance3D {
    /// 实例的世界矩阵；姿态非法（非有限值）或缩放为 0 时返回 `None`（**不**静默退化成单位阵）。
    #[must_use]
    pub fn matrix(&self) -> Option<Mat4> {
        let position = self.position;
        let rotation = self.rotation;
        let scale = self.scale;
        if !position.iter().all(|value| value.is_finite())
            || !rotation.iter().all(|value| value.is_finite())
            || !scale.iter().all(|value| value.is_finite())
            || scale.contains(&0.0)
        {
            return None;
        }
        Some(
            Mat4::translation(Vec3::new(position[0], position[1], position[2]))
                * Mat4::from_quat(Quat {
                    x: rotation[0],
                    y: rotation[1],
                    z: rotation[2],
                    w: rotation[3],
                })
                * Mat4::scale(Vec3::new(scale[0], scale[1], scale[2])),
        )
    }

    /// 缩放是否可用（运动求值不看位置，但仍要求缩放合法）。
    fn scale_usable(&self) -> bool {
        self.scale.iter().all(|value| value.is_finite()) && !self.scale.contains(&0.0)
    }
}

/// 打包实例的步长（字节）：`x, y, z, sx, sy, sz, yaw` 七个**小端 f32**。
///
/// 为什么有这条口径：Lua 侧每帧重建 N 张实例表在 Myrmex 实测 **3.0 µs/只**
/// （`docs/02-balance.md` §21.517），6 万只就是 ≈180 ms/帧。打包成一串字节后宿主只需
/// 一次 `string.pack` 加一次字符串传递，不再逐实例建表、逐键取值；引擎侧按 f32 直接读。
pub const PACKED_INSTANCE_STRIDE: usize = 7 * std::mem::size_of::<f32>();

/// 打包实例表的解析错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackedInstancesError {
    /// 字节数不是 [`PACKED_INSTANCE_STRIDE`] 的整数倍。
    Length { len: usize },
    /// 第 `index` 个实例的 `field` 不是有限数。
    NotFinite { index: usize, field: &'static str },
}

impl std::fmt::Display for PackedInstancesError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Length { len } => write!(
                formatter,
                "packed instance blob is {len} bytes, not a multiple of {PACKED_INSTANCE_STRIDE} \
                 (x,y,z,sx,sy,sz,yaw as f32)"
            ),
            Self::NotFinite { index, field } => {
                write!(
                    formatter,
                    "instance #{index}: `{field}` is not a finite f32"
                )
            }
        }
    }
}

impl std::error::Error for PackedInstancesError {}

/// 打包字节 → 实例表。默认值与表口径一致（位置/缩放照给，姿态由 `yaw` 绕 +Y 生成）。
///
/// 非有限值在这里逐字段拒绝，**不静默跳过**：表口径在 Lua 入口就报错，打包口径没有
/// 逐键检查的机会；放过去的话 `matrices()` 会悄悄丢掉那个实例（数量对不上，很难查）。
pub fn instances_from_packed(bytes: &[u8]) -> Result<Vec<Instance3D>, PackedInstancesError> {
    if !bytes.len().is_multiple_of(PACKED_INSTANCE_STRIDE) {
        return Err(PackedInstancesError::Length { len: bytes.len() });
    }
    const FIELDS: [&str; 7] = ["x", "y", "z", "sx", "sy", "sz", "yaw"];
    let mut instances = Vec::with_capacity(bytes.len() / PACKED_INSTANCE_STRIDE);
    for (index, chunk) in bytes.chunks_exact(PACKED_INSTANCE_STRIDE).enumerate() {
        let mut values = [0.0f32; 7];
        for (slot, raw) in values.iter_mut().zip(chunk.chunks_exact(4)) {
            *slot = f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        }
        for (value, field) in values.iter().zip(FIELDS) {
            if !value.is_finite() {
                return Err(PackedInstancesError::NotFinite { index, field });
            }
        }
        let half = values[6] * 0.5;
        instances.push(Instance3D {
            position: [values[0], values[1], values[2]],
            rotation: [0.0, half.sin(), 0.0, half.cos()],
            scale: [values[3], values[4], values[5]],
            motion: None,
        });
    }
    Ok(instances)
}

/// 路径的弧长参数化：节点累计长度 + 总长（含闭合段）。
#[derive(Clone, Debug, PartialEq)]
pub struct PathArc {
    /// 节点累计长度（`len = 节点数`），`cumulative[0] = 0`。
    cumulative: Vec<f32>,
    /// 总长（闭合时含"末节点 → 首节点"那一段）。
    total: f32,
    /// 是否闭合。
    closed: bool,
}

impl PathArc {
    /// 由路径点建弧长表：跳过与前一点重合的退化点（否则那一段长度为 0，除零）。
    ///
    /// 路径点少于两个、或含非有限值时返回 `None`（调用方据此退回静态表，不静默画错位置）。
    #[must_use]
    pub fn build(points: &[[f32; 3]], closed: bool) -> Option<Self> {
        let mut nodes: Vec<[f32; 3]> = Vec::with_capacity(points.len());
        for point in points {
            if !point.iter().all(|value| value.is_finite()) {
                return None;
            }
            if nodes
                .last()
                .is_some_and(|last| segment_length(*last, *point) <= 0.0)
            {
                continue;
            }
            nodes.push(*point);
        }
        if nodes.len() < 2 {
            return None;
        }
        let mut cumulative = Vec::with_capacity(nodes.len());
        let mut total = 0.0f32;
        cumulative.push(0.0);
        for index in 1..nodes.len() {
            total += segment_length(nodes[index - 1], nodes[index]);
            cumulative.push(total);
        }
        if closed {
            total += segment_length(nodes[nodes.len() - 1], nodes[0]);
        }
        Some(Self {
            cumulative,
            total,
            closed,
        })
    }

    /// 总长（世界单位）。
    #[must_use]
    pub fn total(&self) -> f32 {
        self.total
    }

    /// 节点数（路径点个数，退化点已剔除）。
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.cumulative.len()
    }

    /// 节点累计弧长（`cumulative[0] = 0`，闭合路径不含"末→首"那一段）。
    ///
    /// P3-R244：GPU 求值要把这张表原样搬上显存（CPU 那份 `locate` 的分段口径就在它上面）。
    #[must_use]
    pub fn cumulative(&self) -> &[f32] {
        &self.cumulative
    }

    /// 路径是否闭合。
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// 去重后的路径节点（与弧长表一一对应，退化点已剔除）。
    ///
    /// 与 [`BatchMotion3D::instance_matrix`] 用的是同一份节点表；调用方不必再抄一遍去重规则。
    #[must_use]
    pub fn nodes(&self, points: &[[f32; 3]]) -> Vec<[f32; 3]> {
        motion_nodes(points, self)
    }

    /// 定位弧长 `s` 落在哪一段：返回（段序号，段内比例 `u ∈ [0, 1]`）。
    ///
    /// `s` 会被夹到 `[0, total]`；闭合路径的段 `node_count - 1` 是"末节点 → 首节点"那一段。
    fn locate(&self, s: f32) -> (usize, f32) {
        let last = self.cumulative.len() - 1;
        let mut segment = self.cumulative.partition_point(|value| *value <= s);
        if segment == 0 {
            segment = 1;
        }
        let segment = (segment - 1).min(last);
        let start = self.cumulative[segment];
        let end = if self.closed && segment == last {
            self.total
        } else {
            // 开口路径的最后一段没有"下一节点"：`s` 恰好落在总长上时（弧长被夹到上界）
            // 段号就是最后一段，段的终点就是总长本身。
            self.cumulative
                .get(segment + 1)
                .copied()
                .unwrap_or(self.total)
        };
        let length = end - start;
        let u = if length > 0.0 {
            (s - start) / length
        } else {
            0.0
        };
        (segment, u.clamp(0.0, 1.0))
    }

    /// 第 `segment` 段的两个端点（闭合路径的最后一段首端相接）。
    fn segment_points(&self, segment: usize, nodes: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
        let last = nodes.len() - 1;
        if segment >= last {
            (nodes[last], nodes[0])
        } else {
            (nodes[segment], nodes[segment + 1])
        }
    }
}

/// 两点间的水平距离（忽略 Y：路径流只关心平面上的走向）。
fn segment_length(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = b[0] - a[0];
    let dz = b[2] - a[2];
    (dx * dx + dz * dz).sqrt()
}

impl BatchMotion3D {
    /// 弧长表；路径不合法时 `None`。
    #[must_use]
    pub fn arc(&self) -> Option<PathArc> {
        PathArc::build(&self.path, self.closed)
    }

    /// 单个实例在 `time`（秒）时刻的世界矩阵。
    ///
    /// 位置 = 路径上距起点 `offset + speed × 批次速度 × time` 的那一点；`align` 为真时
    /// 姿态跟随路径的**水平切线**（模型的前向轴按 [`ForwardAxis`] 选 +X 或 +Z 指向
    /// 行进方向），否则沿用实例表里的静态姿态。
    #[must_use]
    pub fn instance_matrix(
        &self,
        arc: &PathArc,
        nodes: &[[f32; 3]],
        instance: &Instance3D,
        time: f32,
    ) -> Option<Mat4> {
        if !instance.scale_usable() {
            return None;
        }
        let params = instance.motion.unwrap_or_default();
        let distance = params.offset + params.speed * self.speed * time;
        let distance = if arc.closed {
            distance.rem_euclid(arc.total)
        } else {
            distance.clamp(0.0, arc.total)
        };
        let (segment, u) = arc.locate(distance);
        let (p0, p1) = arc.segment_points(segment, nodes);
        let position = [
            p0[0] + (p1[0] - p0[0]) * u,
            p0[1] + (p1[1] - p0[1]) * u,
            p0[2] + (p1[2] - p0[2]) * u,
        ];
        let rotation = if params.align {
            let dx = p1[0] - p0[0];
            let dz = p1[2] - p0[2];
            let horizontal = (dx * dx + dz * dz).sqrt();
            if horizontal <= 0.0 {
                return None;
            }
            // 前向轴决定 yaw 的口径：+X 前向 ⇒ 模型 +X 指向切线 ⇒ yaw = atan2(−dz, dx)；
            // +Z 前向 ⇒ 模型 +Z 指向切线 ⇒ yaw = atan2(dx, dz)（两者恒差 π/2）。
            let yaw = match params.forward {
                ForwardAxis::X => (-dz).atan2(dx),
                ForwardAxis::Z => dx.atan2(dz),
            };
            let half = yaw * 0.5;
            [0.0, half.sin(), 0.0, half.cos()]
        } else {
            instance.rotation
        };
        if !rotation.iter().all(|value| value.is_finite()) {
            return None;
        }
        Some(
            Mat4::translation(Vec3::new(position[0], position[1], position[2]))
                * Mat4::from_quat(Quat {
                    x: rotation[0],
                    y: rotation[1],
                    z: rotation[2],
                    w: rotation[3],
                })
                * Mat4::scale(Vec3::new(
                    instance.scale[0],
                    instance.scale[1],
                    instance.scale[2],
                )),
        )
    }
}

/// 局部改段的拒绝原因（P3-R240）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchPatchError {
    /// 批次带程序化运动：位置来自路径，改实例表不会改变画出来的姿态。
    HasMotion,
    /// `start..start+count` 超出已有实例数（局部更新**不扩批**）。
    RangeEnd {
        start: usize,
        count: usize,
        len: usize,
    },
}

impl std::fmt::Display for BatchPatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HasMotion => write!(
                formatter,
                "batch has motion (positions come from the path); rebuild it with mesh.set_batch \
                 instead of patching"
            ),
            Self::RangeEnd { start, count, len } => write!(
                formatter,
                "range {start}..{} is out of bounds for a batch of {len} instances",
                start + count
            ),
        }
    }
}

impl std::error::Error for BatchPatchError {}

/// 一批实例：同一张网格 + 同一份（可选）材质 + N 份实例变换。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstanceBatch3D {
    /// 网格路径（项目相对路径），与 `MeshRenderer.mesh` 同一套解析规则。
    pub mesh: String,
    /// 可选的 `.material` 覆盖；`None` = 用 glTF 内嵌材质。
    pub material: Option<String>,
    /// 实例列表（世界坐标）。
    pub instances: Vec<Instance3D>,
    /// 可选的批次级程序化运动（`None` = 静态批次）。
    pub motion: Option<BatchMotion3D>,
}

impl InstanceBatch3D {
    /// 批次的**静态**实例矩阵；非法实例被跳过（Lua 面在入口就拒绝非法值，这里是兜底）。
    ///
    /// 带运动时本表**不含**运动效果（位置取自实例表）——要看实际画出来的姿态用
    /// [`InstanceBatch3D::matrices_at`]。
    #[must_use]
    pub fn matrices(&self) -> Vec<Mat4> {
        self.instances
            .iter()
            .filter_map(Instance3D::matrix)
            .collect()
    }

    /// 单个实例在 `time`（秒）时刻的世界矩阵（与 `matrices_at` 同一套求值）。
    ///
    /// 供脚本侧读回"引擎算出来的姿态"——读的是同一份求值规则，不是另算一遍。
    #[must_use]
    pub fn instance_matrix_at(&self, index: usize, time: f32) -> Option<Mat4> {
        let instance = self.instances.get(index)?;
        match self.motion.as_ref() {
            Some(motion) => {
                let arc = motion.arc()?;
                let nodes = motion_nodes(&motion.path, &arc);
                motion.instance_matrix(&arc, &nodes, instance, time)
            }
            None => instance.matrix(),
        }
    }

    /// 某个实例在 `time`（秒）时刻的 **yaw**（与生成矩阵用的是同一口径）。
    ///
    /// 口径随实例的前向轴变，因为引擎的 align 本来就按前向轴给 yaw：
    /// - 跟随切线（`align`）⇒ 用**前向轴**的那根模型轴反解（`forward="z"` ⇒ 模型 +Z）；
    /// - 其余（静态实例，或不跟随切线）⇒ 用模型 +X 轴反解，等于实例表里写下的 yaw。
    ///
    /// 脚本侧读回 yaw 时必须拿到"引擎真的用来摆模型的那一个"，否则同一份位姿会
    /// 出现两个差 90° 的 yaw 读数。
    #[must_use]
    pub fn instance_yaw_at(&self, index: usize, time: f32) -> Option<f32> {
        let instance = self.instances.get(index)?;
        let matrix = self.instance_matrix_at(index, time)?;
        let origin = matrix.transform_point(Vec3::new(0.0, 0.0, 0.0)).ok()?;
        let forward = instance
            .motion
            .filter(|motion| motion.align)
            .map_or(ForwardAxis::X, |motion| motion.forward);
        Some(match forward {
            ForwardAxis::X => {
                let axis = matrix.transform_point(Vec3::new(1.0, 0.0, 0.0)).ok()?;
                (-(axis.z - origin.z)).atan2(axis.x - origin.x)
            }
            ForwardAxis::Z => {
                let axis = matrix.transform_point(Vec3::new(0.0, 0.0, 1.0)).ok()?;
                (axis.x - origin.x).atan2(axis.z - origin.z)
            }
        })
    }

    /// 批次的实例矩阵（按 `time` 秒求值程序化运动）。
    ///
    /// 没有运动时与 [`InstanceBatch3D::matrices`] 逐位一致；路径不合法（点数不足/含非有限
    /// 值）时也退回静态表——运动写坏了不该变成"画在别处"，退回静态并让脚本侧看到静止。
    #[must_use]
    pub fn matrices_at(&self, time: f32) -> Vec<Mat4> {
        let Some(motion) = self.motion.as_ref() else {
            return self.matrices();
        };
        let Some(arc) = motion.arc() else {
            return self.matrices();
        };
        let nodes = motion_nodes(&motion.path, &arc);
        self.instances
            .iter()
            .filter_map(|instance| motion.instance_matrix(&arc, &nodes, instance, time))
            .collect()
    }

    /// 就地替换 `start..start+instances.len()` 这一段实例（P3-R240）。
    ///
    /// 数量不变、顺序不变，只有被点到的那几只换值——这是"每帧只改少数几只"的入口：
    /// 整批 `mesh.set_batch` 要重建 N 份实例，局部改段只碰 K 份。
    ///
    /// 不扩批：末尾追加会让"第 k 个"的指代漂移，加实例请重建批次。带运动的批次直接
    /// 拒绝——它的位置来自路径，改实例表不会改变画出来的姿态，静默成功就是谎话。
    pub fn replace_range(
        &mut self,
        start: usize,
        instances: Vec<Instance3D>,
    ) -> Result<(), BatchPatchError> {
        if self.motion.is_some() {
            return Err(BatchPatchError::HasMotion);
        }
        let len = self.instances.len();
        // `start > len` 时即使 count 为 0 也拒绝：那是个越界的游标，不是合法的空段。
        let Some(end) = start.checked_add(instances.len()).filter(|end| *end <= len) else {
            return Err(BatchPatchError::RangeEnd {
                start,
                count: instances.len(),
                len,
            });
        };
        self.instances.splice(start..end, instances);
        Ok(())
    }
}

/// 与弧长表对应的节点列表（跳过退化点，顺序与 `PathArc::cumulative` 一致）。
fn motion_nodes(path: &[[f32; 3]], arc: &PathArc) -> Vec<[f32; 3]> {
    let mut nodes: Vec<[f32; 3]> = Vec::with_capacity(arc.node_count());
    for point in path {
        if nodes
            .last()
            .is_some_and(|last| segment_length(*last, *point) <= 0.0)
        {
            continue;
        }
        nodes.push(*point);
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance_at(x: f32, z: f32) -> Instance3D {
        Instance3D {
            position: [x, 0.0, z],
            ..Instance3D::default()
        }
    }

    /// 按打包口径写一串实例：每只 `x,y,z,sx,sy,sz,yaw`，小端 f32。
    fn pack(rows: &[[f32; 7]]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(rows.len() * PACKED_INSTANCE_STRIDE);
        for row in rows {
            for value in row {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes
    }

    #[test]
    fn packed_instances_match_the_table_encoding_bit_for_bit() {
        let rows = [
            [1.5, 2.0, -3.25, 1.0, 1.0, 1.0, 0.0],
            [-4.0, 0.0, 6.0, 0.5, 0.5, 0.5, std::f32::consts::FRAC_PI_2],
        ];
        let packed = instances_from_packed(&pack(&rows)).expect("解析打包实例");
        // 表口径：位置/缩放照写，yaw 绕 +Y 生成四元数，没有运动参数。
        let expected: Vec<Instance3D> = rows
            .iter()
            .map(|row| {
                let half = row[6] * 0.5;
                Instance3D {
                    position: [row[0], row[1], row[2]],
                    rotation: [0.0, half.sin(), 0.0, half.cos()],
                    scale: [row[3], row[4], row[5]],
                    motion: None,
                }
            })
            .collect();
        assert_eq!(packed, expected);
        // 矩阵逐位相同：打包口径不是"另一套摆法"。
        let table_batch = InstanceBatch3D {
            mesh: "m.glb".to_string(),
            material: None,
            instances: expected,
            motion: None,
        };
        let packed_batch = InstanceBatch3D {
            instances: packed,
            ..table_batch.clone()
        };
        let table_matrices = table_batch.matrices();
        assert_eq!(table_matrices.len(), 2);
        assert_eq!(table_matrices, packed_batch.matrices());
    }

    #[test]
    fn packed_instances_reject_a_partial_record_and_non_finite_fields() {
        let mut bytes = pack(&[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0]]);
        bytes.pop();
        assert_eq!(
            instances_from_packed(&bytes),
            Err(PackedInstancesError::Length {
                len: PACKED_INSTANCE_STRIDE - 1
            })
        );
        let bad = pack(&[[0.0, f32::NAN, 0.0, 1.0, 1.0, 1.0, 0.0]]);
        assert_eq!(
            instances_from_packed(&bad),
            Err(PackedInstancesError::NotFinite {
                index: 0,
                field: "y"
            })
        );
        // 空 blob = 空批次（合法：清空实例但保留实体）。
        assert_eq!(instances_from_packed(&[]), Ok(Vec::new()));
    }

    #[test]
    fn replace_range_patches_only_the_named_slice() {
        let mut batch = InstanceBatch3D {
            mesh: "assets/models/ant.glb".to_owned(),
            material: None,
            instances: vec![
                instance_at(0.0, 0.0),
                instance_at(1.0, 0.0),
                instance_at(2.0, 0.0),
            ],
            motion: None,
        };
        let before = batch.matrices();
        batch
            .replace_range(1, vec![instance_at(9.0, 0.0)])
            .expect("范围内改段");
        let after = batch.matrices();
        assert_eq!(after.len(), 3, "改段不改变数量");
        assert_eq!(after[0], before[0], "第 0 只不动");
        assert_eq!(after[2], before[2], "第 2 只不动");
        let origin = after[1]
            .transform_point(Vec3::new(0.0, 0.0, 0.0))
            .expect("变换点");
        assert!(
            (origin.x - 9.0).abs() < 1e-5,
            "第 1 只换到 x=9，实际 {origin:?}"
        );
        // 空段是合法 no-op（游标落在末尾也行）；不扩批是设计选择。
        batch.replace_range(3, Vec::new()).expect("末尾空段合法");
        assert_eq!(batch.matrices().len(), 3);
    }

    #[test]
    fn replace_range_rejects_out_of_bounds_and_motion_batches() {
        let mut batch = InstanceBatch3D {
            mesh: "assets/models/ant.glb".to_owned(),
            material: None,
            instances: vec![
                instance_at(0.0, 0.0),
                instance_at(1.0, 0.0),
                instance_at(2.0, 0.0),
            ],
            motion: None,
        };
        assert_eq!(
            batch.replace_range(2, vec![instance_at(5.0, 0.0), instance_at(6.0, 0.0)]),
            Err(BatchPatchError::RangeEnd {
                start: 2,
                count: 2,
                len: 3
            })
        );
        // 越界游标即使空段也拒绝：那不是"合法的空段"。
        assert_eq!(
            batch.replace_range(4, Vec::new()),
            Err(BatchPatchError::RangeEnd {
                start: 4,
                count: 0,
                len: 3
            })
        );
        let mut moving = straight_path_batch(1.0, 0.0);
        assert_eq!(
            moving.replace_range(0, vec![instance_at(1.0, 0.0)]),
            Err(BatchPatchError::HasMotion),
            "带运动的批次改实例表不会改变姿态，必须拒绝"
        );
        assert!(
            BatchPatchError::RangeEnd {
                start: 2,
                count: 2,
                len: 3
            }
            .to_string()
            .contains("2..4 is out of bounds for a batch of 3"),
            "错误串要能让脚本侧一眼看懂越界段"
        );
    }

    #[test]
    fn instance_matrix_applies_translation_rotation_scale() {
        let instance = Instance3D {
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [2.0, 2.0, 2.0],
            ..Instance3D::default()
        };
        let matrix = instance.matrix().expect("合法实例");
        let point = matrix
            .transform_point(Vec3::new(1.0, 0.0, 0.0))
            .expect("变换点");
        assert!(
            (point.x - 3.0).abs() < 1e-5
                && (point.y - 2.0).abs() < 1e-5
                && (point.z - 3.0).abs() < 1e-5,
            "缩放 2× + 平移 (1,2,3) ⇒ (3,2,3)，实际 {point:?}"
        );
    }

    #[test]
    fn instance_matrix_rejects_non_finite_and_zero_scale() {
        let mut instance = Instance3D::default();
        instance.position[0] = f32::NAN;
        assert!(instance.matrix().is_none(), "非有限值必须被拒");
        let mut instance = Instance3D::default();
        instance.scale[1] = 0.0;
        assert!(instance.matrix().is_none(), "零缩放必须被拒");
    }

    #[test]
    fn matrices_skip_invalid_instances() {
        let batch = InstanceBatch3D {
            mesh: "assets/models/ant.glb".to_owned(),
            material: None,
            instances: vec![
                Instance3D::default(),
                Instance3D {
                    position: [f32::INFINITY, 0.0, 0.0],
                    ..Instance3D::default()
                },
            ],
            motion: None,
        };
        assert_eq!(batch.matrices().len(), 1, "非法实例不进矩阵表");
    }

    #[test]
    fn no_motion_batch_is_time_invariant() {
        let batch = InstanceBatch3D {
            mesh: "assets/models/ant.glb".to_owned(),
            material: None,
            instances: vec![instance_at(1.0, 2.0), instance_at(3.0, 4.0)],
            motion: None,
        };
        let static_table = batch.matrices();
        for time in [0.0, 1.0, 12.5] {
            let animated = batch.matrices_at(time);
            assert_eq!(animated.len(), static_table.len());
            for (left, right) in animated.iter().zip(static_table.iter()) {
                assert_eq!(left, right, "没有运动时求值结果必须逐位不变");
            }
        }
    }

    fn straight_path_batch(speed: f32, offset: f32) -> InstanceBatch3D {
        InstanceBatch3D {
            mesh: "assets/models/ant.glb".to_owned(),
            material: None,
            instances: vec![Instance3D {
                motion: Some(InstanceMotion3D {
                    offset,
                    speed: 1.0,
                    align: true,
                    forward: ForwardAxis::X,
                }),
                ..Instance3D::default()
            }],
            motion: Some(BatchMotion3D {
                kind: MotionKind::Path,
                path: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
                closed: false,
                speed,
            }),
        }
    }

    #[test]
    fn path_motion_advances_along_the_path() {
        let batch = straight_path_batch(2.0, 0.0);
        let at_zero = batch.matrices_at(0.0);
        let at_one = batch.matrices_at(1.0);
        let origin = |matrix: &Mat4| matrix.transform_point(Vec3::new(0.0, 0.0, 0.0)).unwrap();
        let zero = origin(&at_zero[0]);
        let one = origin(&at_one[0]);
        assert!((zero.x - 0.0).abs() < 1e-4, "t=0 在路径起点，实际 {zero:?}");
        assert!(
            (one.x - 2.0).abs() < 1e-4,
            "速度 2 ⇒ t=1 走到 x=2，实际 {one:?}"
        );
    }

    #[test]
    fn path_motion_respects_per_instance_offset_and_speed() {
        let batch = straight_path_batch(1.0, 3.0);
        let matrices = batch.matrices_at(2.0);
        let point = matrices[0]
            .transform_point(Vec3::new(0.0, 0.0, 0.0))
            .expect("变换点");
        assert!(
            (point.x - 5.0).abs() < 1e-4,
            "偏移 3 + 速度 1×2 ⇒ x=5，实际 {point:?}"
        );
    }

    #[test]
    fn path_motion_aligns_yaw_with_the_horizontal_tangent() {
        let batch = straight_path_batch(1.0, 0.0);
        let matrices = batch.matrices_at(0.5);
        // 模型 +X 轴（世界 (cos θ, 0, −sin θ)）应指向切线 (1, 0, 0) ⇒ θ = 0。
        let axis = matrices[0]
            .transform_point(Vec3::new(1.0, 0.0, 0.0))
            .expect("变换点");
        let origin = matrices[0]
            .transform_point(Vec3::new(0.0, 0.0, 0.0))
            .expect("变换点");
        assert!(
            (axis.x - origin.x - 1.0).abs() < 1e-4 && (axis.z - origin.z).abs() < 1e-4,
            "朝向应沿 +X，实际 {axis:?}（原点 {origin:?}）"
        );
    }

    #[test]
    fn path_motion_forward_z_points_the_models_z_axis_along_the_tangent() {
        let mut batch = straight_path_batch(1.0, 0.0);
        let instance = &mut batch.instances[0];
        let motion = instance.motion.as_mut().expect("实例带运动参数");
        motion.forward = ForwardAxis::Z;
        let matrices = batch.matrices_at(0.5);
        // 模型 +Z 轴（世界 (sin θ, 0, cos θ)）应指向切线 (1, 0, 0) ⇒ θ = π/2。
        let axis = matrices[0]
            .transform_point(Vec3::new(0.0, 0.0, 1.0))
            .expect("变换点");
        let origin = matrices[0]
            .transform_point(Vec3::new(0.0, 0.0, 0.0))
            .expect("变换点");
        assert!(
            (axis.x - origin.x - 1.0).abs() < 1e-4 && (axis.z - origin.z).abs() < 1e-4,
            "+Z 前向时模型 +Z 应沿切线，实际 {axis:?}（原点 {origin:?}）"
        );
        // 与 +X 前向恒差 90°：+X 口径下模型 +X 沿 +X，+Z 口径下模型 +X 转到 −Z。
        let x_axis = matrices[0]
            .transform_point(Vec3::new(1.0, 0.0, 0.0))
            .expect("变换点");
        assert!(
            (x_axis.z - origin.z + 1.0).abs() < 1e-4,
            "+Z 前向时模型 +X 应转到 −Z，实际 {x_axis:?}（原点 {origin:?}）"
        );
    }

    #[test]
    fn closed_path_wraps_around() {
        let batch = InstanceBatch3D {
            mesh: "assets/models/ant.glb".to_owned(),
            material: None,
            instances: vec![Instance3D::default()],
            motion: Some(BatchMotion3D {
                kind: MotionKind::Path,
                path: vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0]],
                closed: true,
                speed: 1.0,
            }),
        };
        let matrices = batch.matrices_at(5.0);
        let point = matrices[0]
            .transform_point(Vec3::new(0.0, 0.0, 0.0))
            .expect("变换点");
        // 闭合回路总长 8（0→4 走 4，4→0 再走 4）⇒ t=5 时在回程上，距末节点 1 ⇒ x=3。
        assert!(
            (point.x - 3.0).abs() < 1e-4,
            "闭合路径应绕回，实际 {point:?}"
        );
    }

    #[test]
    fn invalid_path_falls_back_to_the_static_table() {
        let batch = InstanceBatch3D {
            mesh: "assets/models/ant.glb".to_owned(),
            material: None,
            instances: vec![instance_at(1.0, 2.0)],
            motion: Some(BatchMotion3D {
                kind: MotionKind::Path,
                path: vec![[0.0, 0.0, 0.0]],
                closed: false,
                speed: 1.0,
            }),
        };
        let static_table = batch.matrices();
        let animated = batch.matrices_at(3.0);
        assert_eq!(animated.len(), 1);
        assert_eq!(animated[0], static_table[0], "路径不合法时退回静态表");
    }

    #[test]
    fn instance_yaw_readback_matches_the_yaw_the_engine_used() {
        // 静态实例：读回的 yaw 必须等于实例表里写下的那一个（绕 +Y 的角度）。
        let yaw = 0.7_f32;
        let half = yaw * 0.5;
        let static_batch = InstanceBatch3D {
            mesh: "assets/models/ant.glb".to_owned(),
            material: None,
            instances: vec![Instance3D {
                rotation: [0.0, half.sin(), 0.0, half.cos()],
                ..Instance3D::default()
            }],
            motion: None,
        };
        let read = static_batch
            .instance_yaw_at(0, 0.0)
            .expect("静态实例有 yaw");
        assert!(
            (read - yaw).abs() < 1e-5,
            "静态实例读回的 yaw 应等于写下的 {yaw}，实际 {read}"
        );

        // 跟随切线时读回的 yaw 是**前向轴口径**的那一个：路径沿 +X 走，
        // +X 前向 ⇒ yaw = 0；+Z 前向 ⇒ yaw = π/2（模型 +Z 指向切线）。
        let x_forward = straight_path_batch(1.0, 0.0);
        let x_yaw = x_forward.instance_yaw_at(0, 0.5).expect("+X 前向有 yaw");
        assert!(x_yaw.abs() < 1e-4, "+X 前向时 yaw 应为 0，实际 {x_yaw}");
        let mut z_forward = straight_path_batch(1.0, 0.0);
        let motion = z_forward.instances[0]
            .motion
            .as_mut()
            .expect("实例带运动参数");
        motion.forward = ForwardAxis::Z;
        let z_yaw = z_forward.instance_yaw_at(0, 0.5).expect("+Z 前向有 yaw");
        assert!(
            (z_yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
            "+Z 前向时 yaw 应为 π/2，实际 {z_yaw}"
        );
    }

    #[test]
    fn open_path_stops_at_its_end_without_panicking() {
        // 开口路径：弧长被夹到总长上（末点）时不能越界取"下一节点"。
        let batch = InstanceBatch3D {
            mesh: "assets/models/ant.glb".to_owned(),
            material: None,
            instances: vec![Instance3D {
                motion: Some(InstanceMotion3D {
                    offset: 10.0,
                    speed: 1.0,
                    align: true,
                    forward: ForwardAxis::X,
                }),
                ..Instance3D::default()
            }],
            motion: Some(BatchMotion3D {
                kind: MotionKind::Path,
                path: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
                closed: false,
                speed: 1.0,
            }),
        };
        let matrices = batch.matrices_at(0.0);
        let point = matrices[0]
            .transform_point(Vec3::new(0.0, 0.0, 0.0))
            .expect("变换点");
        assert!(
            (point.x - 10.0).abs() < 1e-4,
            "弧长落在总长上时应当停在末点，实际 {point:?}"
        );
    }

    #[test]
    fn path_arc_skips_degenerate_points() {
        let arc = PathArc::build(&[[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [3.0, 0.0, 0.0]], false)
            .expect("两点成弧");
        assert_eq!(arc.node_count(), 2, "重合点被跳过");
        assert!(
            (arc.total() - 3.0).abs() < 1e-5,
            "总长应为 3，实际 {}",
            arc.total()
        );
    }
}
