//! CPU-side batching for textured 2D Sprites.

use std::{error::Error, fmt};

use crate::core::{
    camera::{Camera2D, CameraBounds, CoordinateError, Viewport},
    par::{Parallelism, map_chunks_in_place},
    sprite::{Sprite, SpriteError, SpriteVertex},
    transform::Transform,
};

/// One Sprite submitted to a batch. Sprites with the same texture and layer are
/// emitted as one draw range; lower layers are always emitted first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteBatchItem {
    pub texture_id: u32,
    pub layer: i32,
    pub sprite: Sprite,
    pub transform: Transform,
}

impl SpriteBatchItem {
    #[must_use]
    pub const fn new(texture_id: u32, layer: i32, sprite: Sprite, transform: Transform) -> Self {
        Self {
            texture_id,
            layer,
            sprite,
            transform,
        }
    }
}

/// A contiguous range in the flattened Sprite vertex buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpriteDrawBatch {
    pub texture_id: u32,
    pub layer: i32,
    /// WP6：这段几何属于哪个渲染层（精灵批 = 世界内容）。`layer` 是**同一层内**的先后，
    /// `render_layer` 是"属于哪个渲染层"——两者职责不同，别混用。
    pub render_layer: crate::core::layer::RenderLayer,
    pub vertex_start: usize,
    pub vertex_count: usize,
}

/// Result of building a Sprite batch for one camera and viewport.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteBatchMesh {
    pub vertices: Vec<SpriteVertex>,
    pub draws: Vec<SpriteDrawBatch>,
}

impl SpriteBatchMesh {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
    #[must_use]
    pub const fn sprite_count(&self) -> usize {
        self.vertices.len() / 6
    }
}

/// 最终 overlay 顶点类型（WP6 单趟化）。
///
/// 核心这一侧只负责算 **NDC 位置 + UV + 颜色**；顶点具体长什么样（有没有额外字段、
/// 对齐如何）由宿主的类型决定。这样 `core` 不必知道 `host::frame::OverlayVertex` 的布局，
/// "`core` 不得依赖 `host`"的分层约束（有架构测试）不被破坏。
///
/// 存在的理由：此前每帧先把顶点物化成一份 `SpriteVertex`，再由宿主逐顶点转成
/// `OverlayVertex`——10k 精灵是 1.9 MB 写 + 1.9 MB 读 + 2.2 MB 写，其中前两项纯属白做。
pub trait OverlayVertexSink: Copy + Send {
    /// 占位顶点：并行分块就地写之前要把输出缓冲按长度开好。
    /// 全零顶点不表示任何合法几何，只是占位。
    fn placeholder() -> Self;

    fn from_ndc(position: [f32; 2], uv: [f32; 2], color: [f32; 4]) -> Self;
}

/// 屏幕像素（左上原点）→ NDC。**只保留这一份实现**：精灵、瓦片、文字都必须走它，
/// 否则"每条路径各自换算"会再次分叉。
#[must_use]
pub fn ndc_from_screen(screen: [f32; 2], viewport: Viewport) -> [f32; 2] {
    [
        screen[0] / viewport.width * 2.0 - 1.0,
        1.0 - screen[1] / viewport.height * 2.0,
    ]
}

/// 世界空间顶点类型（WP6 世界 Sprite 深度通道）。
///
/// 与 [`OverlayVertexSink`] 对称：核心只给**世界坐标 + UV + 颜色**，位置投影交给顶点着色器
/// （用 `Camera2D::view_projection`），因此"2D 内容进深度通道"与 overlay 路径共用同一份
/// quad 数学，却不需要在 CPU 上再走一遍投影。
pub trait WorldSpriteVertexSink: Copy + Send {
    fn placeholder() -> Self;

    fn from_world(position: [f32; 3], uv: [f32; 2], color: [f32; 4]) -> Self;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpriteBatchError {
    Sprite { index: usize, error: SpriteError },
    Coordinate(CoordinateError),
    InvalidTransform { index: usize },
}

impl fmt::Display for SpriteBatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sprite { index, error } => {
                write!(f, "sprite batch item {index} is invalid: {error}")
            }
            Self::Coordinate(error) => error.fmt(f),
            Self::InvalidTransform { index } => {
                write!(f, "sprite batch item {index} has a non-finite transform")
            }
        }
    }
}
impl Error for SpriteBatchError {}
impl From<CoordinateError> for SpriteBatchError {
    fn from(value: CoordinateError) -> Self {
        Self::Coordinate(value)
    }
}

/// Collects Sprite submissions and converts them into sorted draw batches.
#[derive(Clone, Debug, Default)]
pub struct SpriteBatch {
    items: Vec<SpriteBatchItem>,
    /// 世界空间的可见矩形（WP6 收集阶段剔除）。`Some` 时 [`SpriteBatch::push`] 会直接丢掉
    /// 不可能落在其中的精灵——省下的是**每项 80 字节的写入与后续排序**，这两项在小场景里
    /// 是零头，在 5 万精灵档实测占掉整帧的大头（docs/53 §3.9 末）。`None` = 全收。
    cull_bounds: Option<CameraBounds>,
    /// 被剔除的项数（诊断/证据用：为 0 就说明剔除没生效）。
    culled: usize,
}

impl SpriteBatch {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            items: Vec::new(),
            cull_bounds: None,
            culled: 0,
        }
    }
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            items: Vec::with_capacity(capacity),
            cull_bounds: None,
            culled: 0,
        }
    }
    pub fn clear(&mut self) {
        self.items.clear();
        self.culled = 0;
    }
    #[must_use]
    pub const fn len(&self) -> usize {
        self.items.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 设置收集阶段的可见域（世界坐标轴对齐矩形）。`None` 关闭。每帧由宿主按相机设置。
    pub fn set_cull_bounds(&mut self, bounds: Option<CameraBounds>) {
        self.cull_bounds = bounds;
    }

    /// 本帧被收集阶段剔除的精灵数（`clear` 后重新计数）。
    #[must_use]
    pub const fn culled_len(&self) -> usize {
        self.culled
    }

    pub fn push(&mut self, item: SpriteBatchItem) {
        if let Some(bounds) = self.cull_bounds
            && !sprite_intersects_world_bounds(&item, bounds)
        {
            self.culled += 1;
            return;
        }
        self.items.push(item);
    }

    pub fn extend_from_slice(&mut self, items: &[SpriteBatchItem]) {
        self.items.extend_from_slice(items);
    }

    /// Builds stable layer-major, texture-minor batches.
    pub fn build(
        &self,
        camera: Camera2D,
        viewport: Viewport,
    ) -> Result<SpriteBatchMesh, SpriteBatchError> {
        self.build_with_parallelism(camera, viewport, Parallelism::SEQUENTIAL)
    }

    /// 与 [`SpriteBatch::build`] 相同，但允许把**每精灵顶点生成**这一段按分块并行
    /// （WP8）。
    ///
    /// 只有这一步能并行：它读一个只读条目、写自己的 6 个顶点，没有跨精灵状态；排序、
    /// 错误定位与 draw range 合并都保持在调用线程上按原顺序做，因此**并行与串行的输出
    /// 逐字节相同**，失败时报告的仍是排序序中第一个非法条目。`Parallelism::SEQUENTIAL`
    /// 即完全等价于 `build`，这就是回滚开关。
    pub fn build_with_parallelism(
        &self,
        camera: Camera2D,
        viewport: Viewport,
        parallelism: Parallelism,
    ) -> Result<SpriteBatchMesh, SpriteBatchError> {
        let mut ordered: Vec<(usize, &SpriteBatchItem)> = self.items.iter().enumerate().collect();
        ordered.sort_by(|(left_index, left), (right_index, right)| {
            left.layer
                .cmp(&right.layer)
                .then_with(|| left.texture_id.cmp(&right.texture_id))
                .then_with(|| left_index.cmp(right_index))
        });
        // 每精灵固定 6 个顶点：输出按项数 × 6 一次开好，分块就地写入，不做二次拼接。
        let mut vertices = vec![SpriteVertex::default(); ordered.len() * 6];
        let (generated, _parallel) = map_chunks_in_place(
            &ordered,
            parallelism,
            &mut vertices,
            |chunk, out, offset| {
                for (local, (_index, item)) in chunk.iter().enumerate() {
                    if !item.transform.position.x.is_finite()
                        || !item.transform.position.y.is_finite()
                        || !item.transform.rotation.is_finite()
                        || !item.transform.scale.x.is_finite()
                        || !item.transform.scale.y.is_finite()
                    {
                        return Err(offset + local);
                    }
                    let quad = item
                        .sprite
                        .quad(item.transform, camera, viewport)
                        .map_err(|_| offset + local)?;
                    out[local * 6..local * 6 + 6].copy_from_slice(&quad.vertices);
                }
                Ok(())
            },
        );
        if let Err(position) = generated {
            return Err(classify_failure(&ordered, camera, viewport, position));
        }
        let mut draws: Vec<SpriteDrawBatch> = Vec::new();
        let mut start = 0usize;
        for (_, item) in &ordered {
            if let Some(previous) = draws.last_mut()
                && previous.texture_id == item.texture_id
                && previous.layer == item.layer
            {
                previous.vertex_count += 6;
            } else {
                draws.push(SpriteDrawBatch {
                    texture_id: item.texture_id,
                    layer: item.layer,
                    render_layer: crate::core::layer::RenderLayer::World2D,
                    vertex_start: start,
                    vertex_count: 6,
                });
            }
            start += 6;
        }
        Ok(SpriteBatchMesh { vertices, draws })
    }

    /// 单趟产出**最终 overlay 顶点**：与 [`SpriteBatch::build_with_parallelism`] 同一套排序、
    /// 同一套错误语义，但顶点直接以 NDC 追加进 `out`，draw range 追加进 `draws`（起点按
    /// `out` 的当前长度偏移，因此可以安全地给同一缓冲连续追加瓦片、精灵、粒子等多段）。
    ///
    /// 这是 WP6 的"顶点单趟化"：不再先物化一份 `SpriteVertex` 再逐顶点转换。
    ///
    /// `cull` 为真时**先滤掉不可能落在视口内的精灵**（WP6 的 CPU 可见性），再生成顶点：
    /// 过滤发生在生成之前，因此并行分块的"每项固定 6 个顶点"前提不受影响，draw range 也
    /// 不需要按项计数。
    pub fn build_overlay_into<V: OverlayVertexSink>(
        &self,
        camera: Camera2D,
        viewport: Viewport,
        out: &mut Vec<V>,
        draws: &mut Vec<SpriteDrawBatch>,
        parallelism: Parallelism,
        cull: bool,
    ) -> Result<(), SpriteBatchError> {
        let mut ordered: Vec<(usize, &SpriteBatchItem)> = self.items.iter().enumerate().collect();
        ordered.sort_by(|(left_index, left), (right_index, right)| {
            left.layer
                .cmp(&right.layer)
                .then_with(|| left.texture_id.cmp(&right.texture_id))
                .then_with(|| left_index.cmp(right_index))
        });
        if cull {
            ordered.retain(|(_, item)| sprite_may_be_visible(item, camera, viewport));
        }
        if ordered.is_empty() {
            return Ok(());
        }
        let start = out.len();
        let threads = parallelism.effective_threads(ordered.len());
        if threads > 1 {
            // 并行分支必须先把输出开到最终长度（`chunks_mut` 要给出互不重叠的切片）。
            out.resize(start + ordered.len() * 6, V::placeholder());
            let (generated, _) = map_chunks_in_place(
                &ordered,
                parallelism,
                &mut out[start..],
                |chunk, slice, offset| {
                    for (local, (_index, item)) in chunk.iter().enumerate() {
                        let quad = quad_ndc::<V>(item, camera, viewport, offset + local)?;
                        slice[local * 6..local * 6 + 6].copy_from_slice(&quad);
                    }
                    Ok(())
                },
            );
            if let Err(position) = generated {
                out.truncate(start);
                return Err(classify_failure(&ordered, camera, viewport, position));
            }
        } else {
            let mut failure = None;
            for (index, (_original, item)) in ordered.iter().enumerate() {
                match quad_ndc::<V>(item, camera, viewport, index) {
                    Ok(quad) => out.extend_from_slice(&quad),
                    Err(position) => {
                        failure = Some(position);
                        break;
                    }
                }
            }
            if let Some(position) = failure {
                out.truncate(start);
                return Err(classify_failure(&ordered, camera, viewport, position));
            }
        }
        let mut index = start;
        for (_, item) in &ordered {
            if let Some(previous) = draws.last_mut()
                && previous.texture_id == item.texture_id
                && previous.layer == item.layer
                && previous.vertex_start + previous.vertex_count == index
            {
                previous.vertex_count += 6;
            } else {
                draws.push(SpriteDrawBatch {
                    texture_id: item.texture_id,
                    layer: item.layer,
                    render_layer: crate::core::layer::RenderLayer::World2D,
                    vertex_start: index,
                    vertex_count: 6,
                });
            }
            index += 6;
        }
        Ok(())
    }

    /// 单趟产出**世界空间**顶点（WP6 世界 Sprite 深度通道）：位置是世界 `(x, y, z)`，
    /// `z` 来自 `Sprite::z`（默认 0 = 世界平面）。排序与错误语义与
    /// [`SpriteBatch::build_overlay_into`] 完全一致；剔除用世界矩形（不需要相机投影，
    /// 因为投影已经交给顶点着色器）。
    pub fn build_world_into<V: WorldSpriteVertexSink>(
        &self,
        out: &mut Vec<V>,
        draws: &mut Vec<SpriteDrawBatch>,
        cull_bounds: Option<CameraBounds>,
        parallelism: Parallelism,
    ) -> Result<(), SpriteBatchError> {
        let mut ordered: Vec<(usize, &SpriteBatchItem)> = self.items.iter().enumerate().collect();
        // WP6 世界通道的排序契约（"不写深度 + 按 z 排序"）：**先层、再深度、最后纹理**。
        // - `layer` 是作者声明的排序层，语义上必须压过 z（docs/54：排序层 ≠ 深度）；
        // - 同层内按 z **从远到近**（大 z 先画）——半透明走画家算法才能正确叠加；
        // - z 相同的相邻项再按纹理归并，尽量少 draw call；
        // - 最后按原始下标，保证同一输入的输出**稳定可复现**。
        ordered.sort_by(|(left_index, left), (right_index, right)| {
            left.layer
                .cmp(&right.layer)
                .then_with(|| {
                    world_z_sort_key(right.sprite.z).total_cmp(&world_z_sort_key(left.sprite.z))
                })
                .then_with(|| left.texture_id.cmp(&right.texture_id))
                .then_with(|| left_index.cmp(right_index))
        });
        if let Some(bounds) = cull_bounds {
            ordered.retain(|(_, item)| sprite_intersects_world_bounds(item, bounds));
        }
        if ordered.is_empty() {
            return Ok(());
        }
        let start = out.len();
        let threads = parallelism.effective_threads(ordered.len());
        let generate = |item: &SpriteBatchItem| -> Result<[V; 6], usize> {
            let positions = item
                .sprite
                .quad_world(item.transform)
                .map_err(|_| 0_usize)?;
            let z = item.sprite.z;
            let (u0, u1) = if item.sprite.flip_x {
                (item.sprite.uv_max.x, item.sprite.uv_min.x)
            } else {
                (item.sprite.uv_min.x, item.sprite.uv_max.x)
            };
            let (v0, v1) = if item.sprite.flip_y {
                (item.sprite.uv_max.y, item.sprite.uv_min.y)
            } else {
                (item.sprite.uv_min.y, item.sprite.uv_max.y)
            };
            let uv = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
            let vertex = |index: usize| {
                V::from_world(
                    [positions[index][0], positions[index][1], z],
                    uv[index],
                    item.sprite.tint,
                )
            };
            Ok([
                vertex(0),
                vertex(1),
                vertex(2),
                vertex(0),
                vertex(2),
                vertex(3),
            ])
        };
        if threads > 1 {
            out.resize(start + ordered.len() * 6, V::placeholder());
            let (generated, _) = map_chunks_in_place(
                &ordered,
                parallelism,
                &mut out[start..],
                |chunk, slice, offset| {
                    for (local, (_index, item)) in chunk.iter().enumerate() {
                        let quad = generate(item).map_err(|_| offset + local)?;
                        slice[local * 6..local * 6 + 6].copy_from_slice(&quad);
                    }
                    Ok(())
                },
            );
            if let Err(position) = generated {
                out.truncate(start);
                return Err(classify_world_failure(&ordered, position));
            }
        } else {
            for (_original, item) in &ordered {
                match generate(item) {
                    Ok(quad) => out.extend_from_slice(&quad),
                    Err(_) => {
                        out.truncate(start);
                        return Err(classify_world_failure(&ordered, 0));
                    }
                }
            }
        }
        let mut index = start;
        // 分段必须**深度一致**：跨 z 合并会让"一个分段一个 z"的排序/诊断前提失效
        // （分段是渲染器唯一能看到的分组信息，顶点里才带 z）。因此同纹理同层但 z 不同 ⇒ 新分段。
        let mut open_z: Option<f32> = None;
        for (_, item) in &ordered {
            let same_z = open_z.is_some_and(|z| z == item.sprite.z);
            if let Some(previous) = draws.last_mut()
                && same_z
                && previous.texture_id == item.texture_id
                && previous.layer == item.layer
                && previous.vertex_start + previous.vertex_count == index
            {
                previous.vertex_count += 6;
            } else {
                draws.push(SpriteDrawBatch {
                    texture_id: item.texture_id,
                    layer: item.layer,
                    render_layer: crate::core::layer::RenderLayer::World2D,
                    vertex_start: index,
                    vertex_count: 6,
                });
                open_z = Some(item.sprite.z);
            }
            index += 6;
        }
        Ok(())
    }
}

/// 世界 z 的排序键：把 `-0.0` 与 `0.0` 归一，避免它们在排序里被当成不同的深度。
/// NaN 不可能出现（`Sprite` 解析阶段就拒绝非有限 z），这里只是把排序键做成**全序**。
fn world_z_sort_key(z: f32) -> f32 {
    if z == 0.0 { 0.0 } else { z }
}

/// 世界空间路径的失败归因：找出第一个非法项，报出与 overlay 路径同样明确的错误。
fn classify_world_failure(
    ordered: &[(usize, &SpriteBatchItem)],
    position: usize,
) -> SpriteBatchError {
    let (index, item) = ordered
        .get(position)
        .map(|(index, item)| (*index, *item))
        .unwrap_or((0, ordered[0].1));
    match item.sprite.quad_world(item.transform) {
        Ok(_) => SpriteBatchError::InvalidTransform { index },
        Err(error) => SpriteBatchError::Sprite { index, error },
    }
}

/// 精灵是否**可能**与给定的世界矩形相交（收集阶段剔除）。
///
/// 用"中心 + 外接圆半径"：半径 = 世界半对角线 ⇒ **与旋转无关**，旋转、负缩放、任意 pivot 都被
/// 保守覆盖。变换非有限时返回 `true`（保留）：剔除只允许少收视口外的东西，**不能把非法变换
/// 的错误一并吞掉**——那些精灵仍要走原来的报错路径。
fn sprite_intersects_world_bounds(item: &SpriteBatchItem, bounds: CameraBounds) -> bool {
    let transform = item.transform;
    if !transform.position.x.is_finite()
        || !transform.position.y.is_finite()
        || !transform.rotation.is_finite()
        || !transform.scale.x.is_finite()
        || !transform.scale.y.is_finite()
    {
        return true;
    }
    let half_x = item.sprite.size.x * transform.scale.x.abs() * 0.5;
    let half_y = item.sprite.size.y * transform.scale.y.abs() * 0.5;
    let radius = (half_x * half_x + half_y * half_y).sqrt() + 1.0;
    transform.position.x + radius >= bounds.min.x
        && transform.position.x - radius <= bounds.max.x
        && transform.position.y + radius >= bounds.min.y
        && transform.position.y - radius <= bounds.max.y
}

/// 精灵是否**可能**落在视口内（WP6 的 CPU 可见性剔除）。
///
/// 判定用"屏幕中心 + 外接圆半径"：半径 = 世界半对角线 × zoom，因此**与旋转无关**，
/// 旋转、负缩放、任意 pivot 都被保守覆盖（pivot 只改变矩形相对中心的偏移，不超过半对角线）。
/// 中心用 `Camera2D::world_to_screen`——与 `Sprite::quad` 走同一个投影，不会分叉。
///
/// 变换非法或投影失败时返回 `true`（保留）：剔除只允许少画视口外的东西，**绝不能把错误吞掉**
/// ——非法变换仍必须走原来的报错路径。
fn sprite_may_be_visible(item: &SpriteBatchItem, camera: Camera2D, viewport: Viewport) -> bool {
    let Ok(center) = camera.world_to_screen(item.transform.position, viewport) else {
        return true;
    };
    let half_x = item.sprite.size.x * item.transform.scale.x.abs() * 0.5;
    let half_y = item.sprite.size.y * item.transform.scale.y.abs() * 0.5;
    // +1px 抹掉亚像素与采样误差；跨在边缘上的精灵因此一律保留。
    let radius = (half_x * half_x + half_y * half_y).sqrt() * camera.zoom + 1.0;
    center.x + radius >= 0.0
        && center.x - radius <= viewport.width
        && center.y + radius >= 0.0
        && center.y - radius <= viewport.height
}

/// 把"排序序中的失败位置"翻译成既有错误类型。两条顶点生成路径共用同一套分类：
/// 先判变换是否有限，再取精灵自身错误（单条重算，不在热路径上）。
fn classify_failure(
    ordered: &[(usize, &SpriteBatchItem)],
    camera: Camera2D,
    viewport: Viewport,
    position: usize,
) -> SpriteBatchError {
    let (index, item) = ordered[position];
    let transform = item.transform;
    if !transform.position.x.is_finite()
        || !transform.position.y.is_finite()
        || !transform.rotation.is_finite()
        || !transform.scale.x.is_finite()
        || !transform.scale.y.is_finite()
    {
        return SpriteBatchError::InvalidTransform { index };
    }
    match item.sprite.quad(transform, camera, viewport) {
        Err(error) => SpriteBatchError::Sprite { index, error },
        Ok(_) => unreachable!("调用方已在该位置报错"),
    }
}

/// 单个精灵的 6 个 NDC 顶点；失败时返回**排序序位置**（`position`），与并行路径一致。
fn quad_ndc<V: OverlayVertexSink>(
    item: &SpriteBatchItem,
    camera: Camera2D,
    viewport: Viewport,
    position: usize,
) -> Result<[V; 6], usize> {
    let transform = item.transform;
    if !transform.position.x.is_finite()
        || !transform.position.y.is_finite()
        || !transform.rotation.is_finite()
        || !transform.scale.x.is_finite()
        || !transform.scale.y.is_finite()
    {
        return Err(position);
    }
    let quad = item
        .sprite
        .quad(transform, camera, viewport)
        .map_err(|_| position)?;
    Ok(quad.vertices.map(|vertex| {
        V::from_ndc(
            ndc_from_screen(vertex.position, viewport),
            vertex.uv,
            vertex.color,
        )
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        camera::{Camera2D, Viewport},
        sprite::Sprite,
        transform::{Transform, Vec2},
    };

    /// 测试用的世界顶点类型（宿主用 `host::frame::WorldSpriteVertex`；core 不得依赖 host，
    /// 有架构测试把关）。
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct WorldSpriteVertex {
        position: [f32; 3],
        uv: [f32; 2],
        color: [f32; 4],
    }

    impl WorldSpriteVertexSink for WorldSpriteVertex {
        fn placeholder() -> Self {
            Self {
                position: [0.0; 3],
                uv: [0.0; 2],
                color: [0.0; 4],
            }
        }

        fn from_world(position: [f32; 3], uv: [f32; 2], color: [f32; 4]) -> Self {
            Self {
                position,
                uv,
                color,
            }
        }
    }

    /// WP6 世界 Sprite 路径：顶点是**世界坐标**（不是 NDC）、`z` 来自 `Sprite::z`、
    /// 分段与 overlay 路径一致；并行与串行逐字节相同；世界矩形剔除真的会丢项。
    #[test]
    fn world_sprite_builder_emits_world_coordinates_with_z() {
        let sprite = Sprite::new(Vec2::new(10.0, 20.0)).unwrap().with_z(-2.5);
        let transform = Transform {
            position: Vec2::new(100.0, 50.0),
            rotation: 0.0,
            scale: Vec2::new(1.0, 1.0),
        };
        let mut batch = SpriteBatch::new();
        batch.push(SpriteBatchItem::new(7, 3, sprite, transform));
        batch.push(SpriteBatchItem::new(
            7,
            3,
            sprite,
            Transform::from_position(Vec2::new(400.0, 50.0)),
        ));

        let mut vertices: Vec<WorldSpriteVertex> = Vec::new();
        let mut draws = Vec::new();
        batch
            .build_world_into(&mut vertices, &mut draws, None, Parallelism::SEQUENTIAL)
            .unwrap();
        assert_eq!(vertices.len(), 12);
        assert_eq!(
            draws,
            vec![SpriteDrawBatch {
                texture_id: 7,
                layer: 3,
                render_layer: crate::core::layer::RenderLayer::World2D,
                vertex_start: 0,
                vertex_count: 12
            }]
        );
        // 第一个四边形的左上角：世界中心 (100, 50) + 局部 (-5, +10)。
        assert_eq!(vertices[0].position, [95.0, 60.0, -2.5]);
        assert_eq!(vertices[2].position, [105.0, 40.0, -2.5]);
        // UV 与颜色与 overlay 路径同源。
        assert_eq!(vertices[0].uv, [0.0, 0.0]);
        assert_eq!(vertices[0].color, [1.0; 4]);

        // 并行必须与串行逐字节相同（WP8 的不变量在世界上同样成立）。
        let mut parallel_vertices: Vec<WorldSpriteVertex> = Vec::new();
        let mut parallel_draws = Vec::new();
        batch
            .build_world_into(
                &mut parallel_vertices,
                &mut parallel_draws,
                None,
                Parallelism::detected(),
            )
            .unwrap();
        assert_eq!(parallel_vertices, vertices);
        assert_eq!(parallel_draws, draws);

        // 世界矩形剔除：只留矩形内的精灵。
        let bounds = CameraBounds::new(Vec2::new(0.0, 0.0), Vec2::new(200.0, 100.0)).unwrap();
        let mut culled: Vec<WorldSpriteVertex> = Vec::new();
        let mut culled_draws = Vec::new();
        batch
            .build_world_into(
                &mut culled,
                &mut culled_draws,
                Some(bounds),
                Parallelism::SEQUENTIAL,
            )
            .unwrap();
        assert_eq!(culled.len(), 6, "矩形外的精灵应被剔除");
        assert_eq!(culled[0].position, [95.0, 60.0, -2.5]);
    }

    fn setup() -> (Camera2D, Viewport) {
        (
            Camera2D::new(Vec2::zero(), 10.0).unwrap(),
            Viewport::new(100.0, 100.0).unwrap(),
        )
    }
    fn item(texture_id: u32, layer: i32, x: f32) -> SpriteBatchItem {
        SpriteBatchItem::new(
            texture_id,
            layer,
            Sprite::new(Vec2::new(1.0, 1.0)).unwrap(),
            Transform::from_position(Vec2::new(x, 0.0)),
        )
    }

    #[test]
    fn groups_same_texture_and_layer() {
        let (camera, viewport) = setup();
        let mut batch = SpriteBatch::new();
        batch.push(item(2, 0, 0.0));
        batch.push(item(1, 0, 1.0));
        batch.push(item(2, 0, 2.0));
        let mesh = batch.build(camera, viewport).unwrap();
        assert_eq!(mesh.sprite_count(), 3);
        assert_eq!(
            mesh.draws,
            vec![
                SpriteDrawBatch {
                    texture_id: 1,
                    layer: 0,
                    render_layer: crate::core::layer::RenderLayer::World2D,
                    vertex_start: 0,
                    vertex_count: 6
                },
                SpriteDrawBatch {
                    texture_id: 2,
                    layer: 0,
                    render_layer: crate::core::layer::RenderLayer::World2D,
                    vertex_start: 6,
                    vertex_count: 12
                }
            ]
        );
    }
    #[test]
    fn sorts_layers_before_textures_and_is_stable() {
        let (camera, viewport) = setup();
        let mut batch = SpriteBatch::new();
        batch.push(item(9, 1, 0.0));
        batch.push(item(2, -1, 1.0));
        batch.push(item(3, -1, 2.0));
        batch.push(item(2, -1, 3.0));
        let mesh = batch.build(camera, viewport).unwrap();
        assert_eq!(
            mesh.draws
                .iter()
                .map(|draw| (draw.layer, draw.texture_id, draw.vertex_count))
                .collect::<Vec<_>>(),
            vec![(-1, 2, 12), (-1, 3, 6), (1, 9, 6)]
        );
    }
    #[test]
    fn preserves_atlas_uvs_and_tint() {
        let (camera, viewport) = setup();
        let sprite = Sprite::new(Vec2::new(2.0, 2.0))
            .unwrap()
            .with_uv(Vec2::new(0.25, 0.5), Vec2::new(0.75, 1.0))
            .unwrap()
            .with_tint([0.1, 0.2, 0.3, 0.4])
            .unwrap();
        let mut batch = SpriteBatch::new();
        batch.push(SpriteBatchItem::new(4, 0, sprite, Transform::identity()));
        let mesh = batch.build(camera, viewport).unwrap();
        assert_eq!(mesh.vertices[0].uv, [0.25, 0.5]);
        assert_eq!(mesh.vertices[2].uv, [0.75, 1.0]);
        assert_eq!(mesh.vertices[0].color, [0.1, 0.2, 0.3, 0.4]);
    }
    #[test]
    fn rejects_invalid_transform_with_original_index() {
        let (camera, viewport) = setup();
        let mut batch = SpriteBatch::new();
        batch.push(item(1, 0, 0.0));
        batch.push(SpriteBatchItem::new(
            1,
            0,
            Sprite::new(Vec2::new(1.0, 1.0)).unwrap(),
            Transform {
                position: Vec2::new(f32::NAN, 0.0),
                ..Transform::identity()
            },
        ));
        assert_eq!(
            batch.build(camera, viewport),
            Err(SpriteBatchError::InvalidTransform { index: 1 })
        );
    }

    /// WP8 的核心不变量：并行只改变计算发生在哪个线程，**不改变输出**。
    ///
    /// 断言两件事：① 顶点与 draw range 逐字节相同；② 失败时报告的仍是排序序中第一个非法
    /// 条目——并行必须取最小失败位置才做得到，否则错误信息会随线程调度变化。
    #[test]
    fn parallel_build_matches_sequential_byte_for_byte() {
        let (camera, viewport) = setup();
        let mut batch = SpriteBatch::new();
        for index in 0..2_000usize {
            let layer = (index % 3) as i32 - 1;
            batch.push(item((index % 7) as u32, layer, (index % 97) as f32));
        }
        let sequential = batch.build(camera, viewport).unwrap();
        let parallel = batch
            .build_with_parallelism(camera, viewport, Parallelism::with_threads(4))
            .unwrap();
        assert_eq!(sequential.vertices, parallel.vertices);
        assert_eq!(sequential.draws, parallel.draws);

        // 尾部追加一个 NaN 变换：两条路径必须报同一个下标。
        let mut broken = batch.clone();
        broken.push(SpriteBatchItem::new(
            0,
            0,
            Sprite::new(Vec2::new(1.0, 1.0)).unwrap(),
            Transform {
                position: Vec2::new(f32::NAN, 0.0),
                ..Transform::identity()
            },
        ));
        assert_eq!(
            broken.build(camera, viewport),
            broken.build_with_parallelism(camera, viewport, Parallelism::with_threads(4))
        );
    }
}
