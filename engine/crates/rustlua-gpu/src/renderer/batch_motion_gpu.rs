//! P3-R244：路径流实例位姿的 **GPU 求值原语**（`batch_motion.wgsl`）。
//!
//! ## 为什么有这一层
//!
//! P3-R238 把"逐帧 N 份位姿"从 Lua 搬进了引擎，但那次求值仍在 CPU：按 §21.518 的活体读数
//! 0.32~0.42 µs/只外推，6 万只就是 19~25 ms/帧（GDD §16 的标杆指标）。本模块把同一套求值
//! 搬到 compute：输入是批次里**不随时间变**的两份扁平数组（路径节点、逐实例参数），输出是
//! N 份世界矩阵，与 CPU [`InstanceBatch3D::matrices_at`] 同式。
//!
//! ## 与 CPU 的两处刻意差异
//!
//! 1. **非法实例写零矩阵，不压缩**（默认路径）：CPU 的 `matrices_at` 用 `filter_map` 把缩放 0 /
//!    非有限、align 段切线水平长度为 0 的实例**丢掉**（矩阵表长度会变短）；默认路径写零矩阵
//!    （画面同样不出现，但实例数固定为 N）。帧路径接线时这一段必须按"GPU 批单独一条 draw、
//!    不与其它 drawable 合并"处理，否则实例数口径对不上。P3-R251 的**压缩入口**
//!    （[`Renderer::bake_batch_motion_compact`]）改用原子计数器把可见实例写进前缀，并由
//!    indirect 参数携带真实实例数——顺序不稳定，画面等价。P3-R271 起计数器就是参数里的
//!    `instance_count` 本身（`atomicAdd` 的返回值即槽位），不再有单独的收尾 kernel。
//! 2. **剔除是显式入参，默认不做**：P3-R250 的距离球、P3-R255 的视锥六面都由调用方传进来
//!    （后者还要网格局部半径），`None` 时与引入前逐字节相同。剔除走"写零矩阵/不领槽位"这条
//!    退化路径——同一份矩阵同时供主视图/阴影/planar 用，所以这是**全链一致**的取舍。
//!
//! 另有一处**口径空档**：逐实例的 `offset/speed` 非有限时两侧都不校验——CPU 的
//! `filter_map` 只过滤缩放/旋转，会写出 NaN 矩阵；GPU 侧该实例的结果未定义。这是调用方
//! 输入错误，对账门只用合法实例，也不为它定任何口径。
//!
//! ## 提交点与回滚
//!
//! 走既有的 [`Renderer::dispatch_compute`] 接缝：**自己 `submit`**，宿主必须在 `render()`
//! 之前调用才保证 compute 先于图形。无 compute 能力（WebGL2/微信）或
//! `set_compute_enabled(false)` 时显式返回 [`ComputeError::Unsupported`]——不是静默 0。
//! 默认路径（没人调用本模块）一个字节都不写。

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use rustlua_math::math3d::{Mat4, Vec3};
use rustlua_render::instance_batch::{ForwardAxis, InstanceBatch3D};
use wgpu::util::DeviceExt;

use super::{ComputeBinding, ComputeError, ComputeRequest, Renderer};

/// P3-R270：一次烘焙的输入缓冲，按批次身份持久化（原来每次调用都新建 ⇒ 41 批 × 6
/// = 246 次缓冲创建/帧）。P3-R271 去掉 counter（计数器并入 args），降到 5 块。
/// 持住 `Arc` 是为了让「指针作键」安全：缓存自己有一份引用，地址就不会被回收后复用给
/// 别的批次；批次真的没了时 `Arc::strong_count == 1`，由
/// `note_material_bind_frame` 逐帧清掉。
pub(super) struct BakeInputs {
    pub batch: Arc<InstanceBatch3D>,
    pub params: wgpu::Buffer,
    pub nodes: wgpu::Buffer,
    pub cumulative: wgpu::Buffer,
    pub instances: wgpu::Buffer,
    pub args: wgpu::Buffer,
    pub nodes_cap: u64,
    pub cumulative_cap: u64,
    pub instances_cap: u64,
    /// P3-R284：本输入集当前对应的 compute 绑定组身份（`0` = 还没建过）。
    pub bind_serial: u64,
    /// 建立 `bind_serial` 时的实例 arena 代数。绑定 4/5 绑的是**整块** arena
    /// （偏移走 params 的 `base_matrix`），arena 扩容换缓冲后旧绑定组会写到旧缓冲上。
    pub bind_arena_epoch: u64,
}

/// P3-R272：一次烘焙的**静态**输入（只随批次身份变化）。缓存命中时整张表都不构建。
struct BakeStatics {
    nodes: Vec<f32>,
    cumulative: Vec<f32>,
    instances: Vec<f32>,
}

/// 从缓存克隆出来的句柄（克隆是为了立刻放开对缓存的可变借用，后面还要 dispatch）。
#[derive(Clone)]
struct BakeInputSet {
    params: wgpu::Buffer,
    nodes: wgpu::Buffer,
    cumulative: wgpu::Buffer,
    instances: wgpu::Buffer,
    args: wgpu::Buffer,
    /// P3-R284：这一份句柄对应的绑定组身份（与 `BakeInputs::bind_serial` 同值）。
    bind_serial: u64,
}

/// 目标缓冲里一份矩阵的字节数（列主序 4×4 f32）。
pub const BATCH_MOTION_MATRIX_BYTES: u64 = 64;
/// 本模块的 workgroup 宽度（与 shader 的 `@workgroup_size` 同一处口径）。
const WORKGROUP_SIZE: u32 = 64;
const SHADER: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/shaders/batch_motion.wgsl"
));

/// GPU 位姿求值的错误。`Compute` 一档原样透传 compute 接缝的错误（能力/用法/校验）。
#[derive(Debug, thiserror::Error)]
pub enum BatchMotionGpuError {
    /// compute 接缝的错误（无能力、usage 不符、管线/着色器校验失败……）。
    #[error(transparent)]
    Compute(#[from] ComputeError),
    /// 目标缓冲放不下 N 份矩阵。
    #[error(
        "目标缓冲放不下 {instance_count} 份矩阵：偏移 {offset} + {needed} 字节 > 缓冲 {size} 字节"
    )]
    DestinationTooSmall {
        instance_count: usize,
        offset: u64,
        needed: u64,
        size: u64,
    },
    /// 目标偏移不是 64 的倍数（矩阵按整格落位，半格没有语义）。
    #[error("目标偏移 {offset} 不是 {BATCH_MOTION_MATRIX_BYTES} 的倍数")]
    DestinationMisaligned { offset: u64 },
    /// 参数非法（非有限时钟/速度、实例数或节点数超出 u32 等）。
    #[error("批量运动参数非法：{0}")]
    InvalidInput(String),
}

impl From<BatchMotionGpuError> for ComputeError {
    fn from(error: BatchMotionGpuError) -> Self {
        match error {
            BatchMotionGpuError::Compute(error) => error,
            BatchMotionGpuError::DestinationTooSmall { .. }
            | BatchMotionGpuError::DestinationMisaligned { .. }
            | BatchMotionGpuError::InvalidInput(_) => {
                ComputeError::InvalidConfig(error.to_string())
            }
        }
    }
}

/// 目标区的两件事只该有一处口径：64 B 对齐（矩阵整格落位）与容量，返回起始矩阵格下标。
fn matrix_slot_of(
    buffer: &wgpu::Buffer,
    offset: u64,
    needed: u64,
    instance_count: usize,
) -> Result<u32, BatchMotionGpuError> {
    if !offset.is_multiple_of(BATCH_MOTION_MATRIX_BYTES) {
        return Err(BatchMotionGpuError::DestinationMisaligned { offset });
    }
    if offset.saturating_add(needed) > buffer.size() {
        return Err(BatchMotionGpuError::DestinationTooSmall {
            instance_count,
            offset,
            needed,
            size: buffer.size(),
        });
    }
    u32::try_from(offset / BATCH_MOTION_MATRIX_BYTES)
        .map_err(|_| BatchMotionGpuError::InvalidInput("目标偏移超出 u32 矩阵格".to_owned()))
}

/// shader 的 uniform 块（192 字节，与 `batch_motion.wgsl` 的 `Params` 同一布局）。
///
/// 前 48 字节是 P3-R244 的原始布局；`cull_origin`（16 字节对齐）落在 48..64；
/// P3-R266 的密度档第一带落在 64..72（+8 字节填充）；P3-R267 的第二带落在 80..88
/// （+8 字节填充，P3-R273 起前 4 字节是 LOD 第一台阶半径、P3-R274 起后 4 字节是第二
/// 台阶半径）；P3-R255 的视锥六面落在 96..192。
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GpuParams {
    time: f32,
    batch_speed: f32,
    total: f32,
    closed: u32,
    node_count: u32,
    instance_count: u32,
    base_matrix: u32,
    model_scale: f32,
    normal_base_matrix: u32,
    /// P3-R255：1 = 逐实例视锥剔除开；0 = 关（默认）。
    frustum_enabled: u32,
    /// P3-R255：网格局部包围球半径（从局部原点算），逐实例再乘 `max|scale|·model_scale`。
    local_radius: f32,
    /// P3-R273：LOD 侧（`0` = 关，`1` = 近侧 `dist <= 第一台阶`，`2` = 中侧，`3` = 远侧）。
    /// 侧 2/3 的分界见 `lod_distance_far`；没有第二台阶时侧 2 与侧 3 同义。
    lod_side: u32,
    // P3-R250：剔除球心（相机世界位置）+ 半径；半径 <= 0 = 关闭。
    cull_origin: [f32; 3],
    cull_distance: f32,
    /// P3-R266：远程密度档的半径（米）；`<= 0` 或 `thin_stride < 2` = 关闭。
    thin_distance: f32,
    /// P3-R266：远区保留 `index % thin_stride == 0` 的实例（`1` = 关）。
    thin_stride: u32,
    /// P3-R273：LOD 第一台阶半径（米，离 `cull_origin`）；`<= 0` 或 `lod_side == 0` = 关。
    lod_distance: f32,
    /// P3-R274：LOD 第二台阶半径（米）；`<= lod_distance` = 没有第二台阶（中/远同侧）。
    lod_distance_far: f32,
    /// P3-R267：第二远带的半径（米）；`<= 0` 或 `thin_stride2 < 2` = 关闭。
    thin_distance2: f32,
    /// P3-R267：第二远带保留 `index % thin_stride2 == 0`（与第一带取交集；`1` = 关）。
    thin_stride2: u32,
    _pad2: [f32; 2],
    /// P3-R255：主视图视锥六面（归一化，[nx, ny, nz, d]）。关档时全 0。
    frustum: [[f32; 4]; 6],
}

impl Renderer {
    /// 把一批**带路径运动**的实例在 `time` 秒的世界矩阵用 compute 写进 `destination`
    /// （从 `destination_offset` 起，64 B/实例，列主序）。
    ///
    /// 返回写入的实例数。以下情形返回 `Ok(0)`（没有可求值的东西，调用方回退 CPU）：
    /// 批次没有运动、路径不合法（点数不足/含非有限值）、批次为空。
    ///
    /// # Errors
    ///
    /// 无 compute 能力、目标缓冲太小/未对齐、时钟或批次速度非有限、着色器校验失败。
    pub fn bake_batch_motion(
        &mut self,
        batch: &InstanceBatch3D,
        time: f32,
        destination: &wgpu::Buffer,
        destination_offset: u64,
    ) -> Result<u32, BatchMotionGpuError> {
        self.bake_batch_motion_with_normals(batch, time, destination, destination_offset, None, 1.0)
    }

    /// [`Self::bake_batch_motion`] 的 PBR 版本：`normals` 为 `Some((buffer, offset))` 时走
    /// `main_normals` 入口，把逐实例**法线矩阵**（模型矩阵 3x3 的逆转置，即 CPU
    /// `pbr::normal_matrix` 那一份）同时写进该缓冲；`model_scale` 与 CPU `scaled_model`
    /// 同式乘进 3x3（只动旋转/缩放三列，不动平移）。
    ///
    /// 两个目标区的偏移都按 64 B 对齐；矩阵与法线的格下标各自算（可以不相邻）。
    ///
    /// # Errors
    ///
    /// 同 [`Self::bake_batch_motion`]，外加法线目标区太小/未对齐、`model_scale` 非有限。
    pub fn bake_batch_motion_with_normals(
        &mut self,
        batch: &InstanceBatch3D,
        time: f32,
        destination: &wgpu::Buffer,
        destination_offset: u64,
        normals: Option<(&wgpu::Buffer, u64)>,
        model_scale: f32,
    ) -> Result<u32, BatchMotionGpuError> {
        self.bake_batch_motion_with_normals_and_cull(
            batch,
            time,
            destination,
            destination_offset,
            normals,
            model_scale,
            None,
        )
    }

    /// P3-R250：[`Self::bake_batch_motion_with_normals`] + 逐实例距离剔除。
    ///
    /// `cull = Some((球心, 半径))` 时，求值出的世界位置离球心超过半径的实例写**零矩阵**
    /// （与非法实例同一条退化路径，实例数仍为 N）；`None`/半径 <= 0 = 关闭，逐字节不变。
    /// 剔除判在**世界位置**上（不是包围球），因此同一份矩阵在主视图/阴影/planar 里一致。
    ///
    /// # Errors
    ///
    /// 同 [`Self::bake_batch_motion_with_normals`]，外加剔除球心/半径非有限。
    #[allow(clippy::too_many_arguments)]
    pub fn bake_batch_motion_with_normals_and_cull(
        &mut self,
        batch: &InstanceBatch3D,
        time: f32,
        destination: &wgpu::Buffer,
        destination_offset: u64,
        normals: Option<(&wgpu::Buffer, u64)>,
        model_scale: f32,
        cull: Option<(Vec3, f32)>,
    ) -> Result<u32, BatchMotionGpuError> {
        Ok(self
            .bake_batch_motion_inner(
                batch,
                time,
                destination,
                destination_offset,
                normals,
                model_scale,
                cull,
                None,
                None,
                0.0,
                None,
                None,
                None,
                false,
                None,
            )?
            .0)
    }

    /// P3-R251：[`Self::bake_batch_motion_with_normals_and_cull`] 的**压缩**版本：可见实例
    /// 用原子计数器写进目标区前缀（顺序不稳定），并由一个小 kernel 把存活数写进
    /// indirect 参数（`u32[0] = index_count`、`u32[1] = instance_count`，其余 0）。
    /// 调用方用 `draw_indexed_indirect` 按返回值里的参数缓冲绘制。
    ///
    /// P3-R255：`frustum = Some(六面)` 时再叠一层逐实例视锥剔除（保守包围球 =
    /// `local_radius × max|scale| × model_scale`）；被剔实例连槽位都不领。`None` = 关。
    ///
    /// 返回 `Ok(None)` 与 `Ok(0)` 同义（没有可求值的东西，调用方回退 CPU）。
    ///
    /// # Errors
    ///
    /// 同 [`Self::bake_batch_motion_with_normals_and_cull`]，外加视锥/局部半径非有限。
    #[allow(clippy::too_many_arguments)]
    pub fn bake_batch_motion_compact(
        &mut self,
        batch: &InstanceBatch3D,
        time: f32,
        destination: &wgpu::Buffer,
        destination_offset: u64,
        normals: Option<(&wgpu::Buffer, u64)>,
        model_scale: f32,
        cull: Option<(Vec3, f32)>,
        index_count: u32,
        frustum: Option<[[f32; 4]; 6]>,
        local_radius: f32,
        thin: Option<(Vec3, f32, u32)>,
        thin_far: Option<(Vec3, f32, u32)>,
    ) -> Result<Option<wgpu::Buffer>, BatchMotionGpuError> {
        self.bake_batch_motion_compact_with_lod(
            batch,
            time,
            destination,
            destination_offset,
            normals,
            model_scale,
            cull,
            index_count,
            frustum,
            local_radius,
            thin,
            thin_far,
            None,
        )
    }

    /// P3-R273：[`Self::bake_batch_motion_compact`] + 网格 LOD 侧选择。
    /// P3-R274：扩成三档——`lod = Some((球心, 第一台阶, 第二台阶或 0, 侧))`，侧取值
    /// `1` = 近（`dist <= 第一台阶`）/ `2` = 中 / `3` = 远。相邻两档严格互斥、合起来
    /// 完备（侧 1 ∪ 侧 2 ∪ 侧 3 = 开 LOD 前会画出来的全部存活实例）；没有第二台阶
    /// （第二半径 <= 第一半径）时侧 2 与侧 3 同义，和 R273 的两档行为逐字节一致。
    /// 判距与剔除档共用同一个世界位置口径，因此同一份矩阵在主视图/阴影/planar 里一致。
    /// `None` 与旧入口逐字节同义。
    ///
    /// # Errors
    ///
    /// 同 [`Self::bake_batch_motion_compact`]，外加 LOD 球心/半径非有限、侧不在 1..=3、
    /// 侧 3 却没有合法的第二台阶。
    #[allow(clippy::too_many_arguments)]
    pub fn bake_batch_motion_compact_with_lod(
        &mut self,
        batch: &InstanceBatch3D,
        time: f32,
        destination: &wgpu::Buffer,
        destination_offset: u64,
        normals: Option<(&wgpu::Buffer, u64)>,
        model_scale: f32,
        cull: Option<(Vec3, f32)>,
        index_count: u32,
        frustum: Option<[[f32; 4]; 6]>,
        local_radius: f32,
        thin: Option<(Vec3, f32, u32)>,
        thin_far: Option<(Vec3, f32, u32)>,
        lod: Option<(Vec3, f32, f32, u32)>,
    ) -> Result<Option<wgpu::Buffer>, BatchMotionGpuError> {
        Ok(self
            .bake_batch_motion_inner(
                batch,
                time,
                destination,
                destination_offset,
                normals,
                model_scale,
                cull,
                Some(index_count),
                frustum,
                local_radius,
                thin,
                thin_far,
                None,
                false,
                lod,
            )?
            .1)
    }

    /// P3-R270：[`Self::bake_batch_motion_compact`] 的**帧路径**版本。多收一个批次身份
    /// （`Arc`）：输入缓冲按身份持久化、每帧只 `write_buffer`，dispatch 录进本帧共享
    /// 编码器（由 `note_material_bind_frame` 一次提交）。P3-R271 起一次调用只录一次
    /// dispatch（收尾 kernel 已折进主 kernel）。不带身份的公开入口逐字节保持原
    /// 语义（每次新建缓冲、每次自己 submit），对账门/回读走的仍是那条。
    /// P3-R273：[`Self::bake_batch_motion_compact_cached`] + 网格 LOD 侧选择。
    /// P3-R274：侧取值 1..=3（近/中/远），第二台阶半径随参数走。
    ///
    /// LOD 侧进缓存键：同一条批次会有 2~3 份输入集，各带自己的 `params`（球心/半径/侧）
    /// 与 `args` 缓冲——共用一份会被后一条 draw 覆盖、两条 dispatch 读到同一份参数。
    ///
    /// P3-R284：`destination_is_arena` 由调用方声明「目标缓冲就是本帧实例 arena」。只有
    /// 这一档才缓存 compute 绑定组——wgpu 的 `Buffer` 没有可哈希的稳定 id，外部调用方每次
    /// 新建目标缓冲时渲染器无从分辨，缓存会让第二帧写进上一帧那块缓冲。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bake_batch_motion_compact_cached_with_lod(
        &mut self,
        batch: &Arc<InstanceBatch3D>,
        time: f32,
        destination: &wgpu::Buffer,
        destination_offset: u64,
        normals: Option<(&wgpu::Buffer, u64)>,
        model_scale: f32,
        cull: Option<(Vec3, f32)>,
        index_count: u32,
        frustum: Option<[[f32; 4]; 6]>,
        local_radius: f32,
        thin: Option<(Vec3, f32, u32)>,
        thin_far: Option<(Vec3, f32, u32)>,
        lod: Option<(Vec3, f32, f32, u32)>,
        destination_is_arena: bool,
    ) -> Result<Option<wgpu::Buffer>, BatchMotionGpuError> {
        Ok(self
            .bake_batch_motion_inner(
                batch,
                time,
                destination,
                destination_offset,
                normals,
                model_scale,
                cull,
                Some(index_count),
                frustum,
                local_radius,
                thin,
                thin_far,
                Some(batch),
                destination_is_arena,
                lod,
            )?
            .1)
    }

    /// P3-R270：批次身份的缓存键。P3-R272 起身份是**稳定**的：世界组件本身就是
    /// `Arc<InstanceBatch3D>`，提取层递过来的是同一个分配；局部改段走 `Arc::make_mut`，
    /// 真有改动就换地址 ⇒ 键自然失效。不再依赖"每帧重建一个等长 Arc、分配器恰好复用
    /// 同一地址"这种巧合。
    fn bake_input_key(
        batch: &Arc<InstanceBatch3D>,
        compact_index_count: Option<u32>,
        normals: bool,
        lod_side: u32,
    ) -> (usize, u32, bool, u32) {
        (
            Arc::as_ptr(batch) as usize,
            compact_index_count.unwrap_or(0),
            normals,
            lod_side,
        )
    }

    /// P3-R270：取（或建）该批次的持久输入缓冲，把本帧内容写进去，返回克隆句柄。
    ///
    /// P3-R272：`statics` 为 `None` = **缓存命中**——nodes/cumulative/instances 只随批次
    /// 身份变化，一个字节都不重传（调用方连这三张表的构建都省了），每帧只写 `params`
    /// （时间/剔除球）与 `args`（压缩计数必须复位）。`Some` = 首建或身份变化，按容量
    /// 重建/扩容后整批上传。容量不足才扩容，稳态零创建。
    fn bake_inputs_for(
        &mut self,
        batch: &Arc<InstanceBatch3D>,
        compact_index_count: Option<u32>,
        normals: bool,
        lod_side: u32,
        params: &GpuParams,
        statics: Option<&BakeStatics>,
    ) -> BakeInputSet {
        let key = Self::bake_input_key(batch, compact_index_count, normals, lod_side);
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        if !self.bake_inputs.contains_key(&key) {
            let statics = statics.expect("插入输入缓冲必须带静态输入");
            let nodes_size = (statics.nodes.len() * 4) as u64;
            let cumulative_size = (statics.cumulative.len() * 4) as u64;
            let instances_size = (statics.instances.len() * 4) as u64;
            let device = &self.device;
            let make = |label: &str, size: u64, usage: wgpu::BufferUsages| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: size.max(4),
                    usage,
                    mapped_at_creation: false,
                })
            };
            self.bake_inputs.insert(
                key,
                BakeInputs {
                    batch: batch.clone(),
                    params: make(
                        "rustlua-batch-motion-params",
                        std::mem::size_of::<GpuParams>() as u64,
                        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    ),
                    nodes: make("rustlua-batch-motion-nodes", nodes_size, storage),
                    cumulative: make("rustlua-batch-motion-cumulative", cumulative_size, storage),
                    instances: make("rustlua-batch-motion-instances", instances_size, storage),
                    args: make(
                        "rustlua-batch-motion-compact-args",
                        20,
                        storage | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_SRC,
                    ),
                    nodes_cap: nodes_size.max(4),
                    cumulative_cap: cumulative_size.max(4),
                    instances_cap: instances_size.max(4),
                    bind_serial: 0,
                    bind_arena_epoch: 0,
                },
            );
            self.bake_input_allocs += 5;
        }
        let entry = self
            .bake_inputs
            .get_mut(&key)
            .expect("bake inputs just inserted");
        let mut stale_serial = 0u64;
        if let Some(statics) = statics {
            let grow = |buffer: &mut wgpu::Buffer, cap: &mut u64, size: u64, label: &str| {
                if size > *cap {
                    *buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(label),
                        size,
                        usage: storage,
                        mapped_at_creation: false,
                    });
                    *cap = size;
                }
            };
            let caps_before = (entry.nodes_cap, entry.cumulative_cap, entry.instances_cap);
            grow(
                &mut entry.nodes,
                &mut entry.nodes_cap,
                (statics.nodes.len() * 4) as u64,
                "rustlua-batch-motion-nodes",
            );
            grow(
                &mut entry.cumulative,
                &mut entry.cumulative_cap,
                (statics.cumulative.len() * 4) as u64,
                "rustlua-batch-motion-cumulative",
            );
            grow(
                &mut entry.instances,
                &mut entry.instances_cap,
                (statics.instances.len() * 4) as u64,
                "rustlua-batch-motion-instances",
            );
            self.bake_input_allocs += u64::from(entry.nodes_cap != caps_before.0)
                + u64::from(entry.cumulative_cap != caps_before.1)
                + u64::from(entry.instances_cap != caps_before.2);
            // P3-R284：扩容换了缓冲 ⇒ 旧绑定组绑的是旧缓冲，身份必须作废。
            if entry.nodes_cap != caps_before.0
                || entry.cumulative_cap != caps_before.1
                || entry.instances_cap != caps_before.2
            {
                stale_serial = entry.bind_serial;
                entry.bind_serial = 0;
            }
            self.queue
                .write_buffer(&entry.nodes, 0, bytemuck::cast_slice(&statics.nodes));
            self.queue.write_buffer(
                &entry.cumulative,
                0,
                bytemuck::cast_slice(&statics.cumulative),
            );
            self.queue.write_buffer(
                &entry.instances,
                0,
                bytemuck::cast_slice(&statics.instances),
            );
            self.bake_static_uploads += 3;
        }
        self.queue
            .write_buffer(&entry.params, 0, bytemuck::bytes_of(params));
        let mut args = [0u32; 5];
        args[0] = compact_index_count.unwrap_or(0);
        self.queue
            .write_buffer(&entry.args, 0, bytemuck::cast_slice(&args));
        // P3-R284：绑定组身份 = 输入集身份 + 实例 arena 代数。绑定 4/5 绑的是**整块**
        // arena（矩阵格偏移走 params），arena 扩容换缓冲后旧绑定组会写进已被替换的那一块
        // ——画面不动也不报错，正是 compute 接缝文档第 2 条硬约束要防的形状。
        if entry.bind_serial == 0 || entry.bind_arena_epoch != self.instance_arena_epoch {
            if entry.bind_serial != 0 {
                stale_serial = entry.bind_serial;
            }
            self.bake_bind_serials = self.bake_bind_serials.wrapping_add(1);
            entry.bind_serial = self.bake_bind_serials;
            entry.bind_arena_epoch = self.instance_arena_epoch;
        }
        if stale_serial != 0 {
            self.compute_state.retire_bind_group(stale_serial);
        }
        BakeInputSet {
            params: entry.params.clone(),
            nodes: entry.nodes.clone(),
            cumulative: entry.cumulative.clone(),
            instances: entry.instances.clone(),
            args: entry.args.clone(),
            bind_serial: entry.bind_serial,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn bake_batch_motion_inner(
        &mut self,
        batch: &InstanceBatch3D,
        time: f32,
        destination: &wgpu::Buffer,
        destination_offset: u64,
        normals: Option<(&wgpu::Buffer, u64)>,
        model_scale: f32,
        cull: Option<(Vec3, f32)>,
        compact_index_count: Option<u32>,
        frustum: Option<[[f32; 4]; 6]>,
        local_radius: f32,
        thin: Option<(Vec3, f32, u32)>,
        thin_far: Option<(Vec3, f32, u32)>,
        cache: Option<&Arc<InstanceBatch3D>>,
        destination_is_arena: bool,
        lod: Option<(Vec3, f32, f32, u32)>,
    ) -> Result<(u32, Option<wgpu::Buffer>), BatchMotionGpuError> {
        if !model_scale.is_finite() {
            return Err(BatchMotionGpuError::InvalidInput(format!(
                "model_scale={model_scale} 必须是有限值"
            )));
        }
        if frustum.is_some() && (!local_radius.is_finite() || local_radius < 0.0) {
            return Err(BatchMotionGpuError::InvalidInput(format!(
                "local_radius={local_radius} 必须是非负有限值"
            )));
        }
        if let Some(planes) = frustum
            && planes.iter().flatten().any(|value| !value.is_finite())
        {
            return Err(BatchMotionGpuError::InvalidInput(
                "视锥平面必须是有限值".to_owned(),
            ));
        }
        if let Some((origin, distance)) = cull
            && (!origin.x.is_finite()
                || !origin.y.is_finite()
                || !origin.z.is_finite()
                || !distance.is_finite())
        {
            return Err(BatchMotionGpuError::InvalidInput(format!(
                "剔除球心 {origin:?} / 半径 {distance} 必须是有限值"
            )));
        }
        for band in [thin, thin_far].into_iter().flatten() {
            let (origin, distance, stride) = band;
            if !origin.x.is_finite()
                || !origin.y.is_finite()
                || !origin.z.is_finite()
                || !distance.is_finite()
                || stride == 0
            {
                return Err(BatchMotionGpuError::InvalidInput(format!(
                    "远程密度档 球心 {origin:?} / 半径 {distance} / 步长 {stride} 非法"
                )));
            }
        }
        if let Some((origin, distance, far_distance, side)) = lod {
            if !origin.x.is_finite()
                || !origin.y.is_finite()
                || !origin.z.is_finite()
                || !distance.is_finite()
                || distance < 0.0
            {
                return Err(BatchMotionGpuError::InvalidInput(format!(
                    "LOD 球心 {origin:?} / 半径 {distance} 必须是非负有限值"
                )));
            }
            if !far_distance.is_finite() || far_distance < 0.0 {
                return Err(BatchMotionGpuError::InvalidInput(format!(
                    "LOD 第二台阶半径 {far_distance} 必须是非负有限值"
                )));
            }
            if !(1..=3).contains(&side) {
                return Err(BatchMotionGpuError::InvalidInput(format!(
                    "LOD 侧 {side} 必须是 1（近）/ 2（中）/ 3（远）"
                )));
            }
            if far_distance > 0.0 && far_distance <= distance {
                return Err(BatchMotionGpuError::InvalidInput(format!(
                    "LOD 第二台阶半径 {far_distance} 必须大于第一台阶 {distance}"
                )));
            }
            if side == 3 && far_distance <= distance {
                return Err(BatchMotionGpuError::InvalidInput(format!(
                    "LOD 远侧需要合法的第二台阶（第一 {distance} / 第二 {far_distance}）"
                )));
            }
        }
        let Some(motion) = batch.motion.as_ref() else {
            return Ok((0, None));
        };
        let Some(arc) = motion.arc() else {
            return Ok((0, None));
        };
        let instances = &batch.instances;
        if instances.is_empty() {
            return Ok((0, None));
        }
        if !time.is_finite() || !motion.speed.is_finite() {
            return Err(BatchMotionGpuError::InvalidInput(format!(
                "time={time} / speed={} 必须是有限值",
                motion.speed
            )));
        }
        if instances.len() > u32::MAX as usize || arc.node_count() > u32::MAX as usize {
            return Err(BatchMotionGpuError::InvalidInput(
                "实例数或路径节点数超出 u32".to_owned(),
            ));
        }
        let needed = instances.len() as u64 * BATCH_MOTION_MATRIX_BYTES;
        let base_matrix = matrix_slot_of(destination, destination_offset, needed, instances.len())?;
        let normal_base_matrix = match normals {
            Some((buffer, offset)) => matrix_slot_of(buffer, offset, needed, instances.len())?,
            None => 0,
        };

        // P3-R272：命中缓存时静态输入一个字节都不重建（6 万只那 12 张表 ≈3 MB/帧的
        // 分配 + 填充全免）。身份失效只有两条路：世界组件换了新 `Arc`（`mesh.set_batch`）
        // 或 `Arc::make_mut` 因局部改段而拷贝（`mesh.set_batch_range`）——两者都换地址。
        // P3-R273：LOD 侧也进键——2~3 条 draw 共用批次身份但参数不同，必须各有一份。
        let lod_side = lod.map_or(0, |(_, _, _, side)| side);
        let cache_key = cache.map(|batch| {
            Self::bake_input_key(batch, compact_index_count, normals.is_some(), lod_side)
        });
        let upload_static = cache_key.is_none_or(|key| !self.bake_inputs.contains_key(&key));
        let statics = upload_static.then(|| {
            // 路径节点与累计弧长：与 CPU `matrices_at` 用的是同一份去重/累计口径。
            let nodes = arc.nodes(&motion.path);
            let mut node_data: Vec<f32> = Vec::with_capacity(nodes.len() * 4);
            for node in &nodes {
                node_data.extend_from_slice(&[node[0], node[1], node[2], 0.0]);
            }
            // 逐实例参数：offset / speed / flags / 静态旋转 / 缩放。
            let mut instance_data: Vec<f32> = Vec::with_capacity(instances.len() * 12);
            for instance in instances {
                let motion = instance.motion.unwrap_or_default();
                let mut flags = 0u32;
                if motion.align {
                    flags |= 1;
                }
                if motion.forward == ForwardAxis::Z {
                    flags |= 2;
                }
                instance_data.extend_from_slice(&[
                    motion.offset,
                    motion.speed,
                    flags as f32,
                    0.0,
                    instance.rotation[0],
                    instance.rotation[1],
                    instance.rotation[2],
                    instance.rotation[3],
                    instance.scale[0],
                    instance.scale[1],
                    instance.scale[2],
                    0.0,
                ]);
            }
            BakeStatics {
                nodes: node_data,
                cumulative: arc.cumulative().to_vec(),
                instances: instance_data,
            }
        });
        let params = GpuParams {
            time,
            batch_speed: motion.speed,
            total: arc.total(),
            closed: u32::from(arc.is_closed()),
            node_count: arc.node_count() as u32,
            instance_count: instances.len() as u32,
            base_matrix,
            model_scale,
            normal_base_matrix,
            frustum_enabled: u32::from(frustum.is_some()),
            local_radius: if frustum.is_some() { local_radius } else { 0.0 },
            lod_side,
            // P3-R250：None 或半径 <= 0 ⇒ 0，shader 侧判定 `> 0.0` 才剔除。
            // P3-R266：远程密度档与距离剔除共用同一个球心（相机世界位置）——thin 单独开时
            // 也要把这个原点带过去，否则 shader 会拿 [0,0,0] 当相机量距离。
            // P3-R273：LOD 同理（单开 LOD 时剔除档全关，也要真球心）。
            cull_origin: cull
                .map(|(origin, _)| origin)
                .or(thin.map(|(origin, _, _)| origin))
                .or(lod.map(|(origin, _, _, _)| origin))
                .map_or([0.0; 3], |origin| [origin.x, origin.y, origin.z]),
            cull_distance: cull.map_or(0.0, |(_, distance)| distance.max(0.0)),
            thin_distance: thin.map_or(0.0, |(_, distance, _)| distance.max(0.0)),
            thin_stride: thin.map_or(1, |(_, _, stride)| stride.max(1)),
            lod_distance: lod.map_or(0.0, |(_, distance, _, _)| distance.max(0.0)),
            lod_distance_far: lod.map_or(0.0, |(_, _, far_distance, _)| far_distance.max(0.0)),
            thin_distance2: thin_far.map_or(0.0, |(_, distance, _)| distance.max(0.0)),
            thin_stride2: thin_far.map_or(1, |(_, _, stride)| stride.max(1)),
            _pad2: [0.0; 2],
            frustum: frustum.unwrap_or([[0.0; 4]; 6]),
        };

        // P3-R270：帧路径（带批次身份）复用持久缓冲、每帧只 write_buffer；外部调用方
        // （对账门/回读）仍每次新建，逐字节保持原语义。
        let cached = cache.map(|batch| {
            self.bake_inputs_for(
                batch,
                compact_index_count,
                normals.is_some(),
                lod_side,
                &params,
                statics.as_ref(),
            )
        });
        let uniform = match cached.as_ref() {
            Some(set) => set.params.clone(),
            None => self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("rustlua-batch-motion-params"),
                    contents: bytemuck::bytes_of(&params),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                }),
        };
        // 非缓存入口（对账门/回读/`--no-bake-cache`）每次新建，必须现成的静态表。
        let plain_statics = || statics.as_ref().expect("非缓存入口必须构建静态输入");
        let node_buffer = match cached.as_ref() {
            Some(set) => set.nodes.clone(),
            None => self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("rustlua-batch-motion-nodes"),
                    contents: bytemuck::cast_slice(&plain_statics().nodes),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                }),
        };
        let cumulative_buffer = match cached.as_ref() {
            Some(set) => set.cumulative.clone(),
            None => self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("rustlua-batch-motion-cumulative"),
                    contents: bytemuck::cast_slice(&plain_statics().cumulative),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                }),
        };
        let instance_buffer = match cached.as_ref() {
            Some(set) => set.instances.clone(),
            None => self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("rustlua-batch-motion-instances"),
                    contents: bytemuck::cast_slice(&plain_statics().instances),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                }),
        };

        let mut bindings = vec![
            ComputeBinding::Uniform {
                binding: 0,
                buffer: &uniform,
            },
            ComputeBinding::Storage {
                binding: 1,
                buffer: &node_buffer,
                read_only: true,
            },
            ComputeBinding::Storage {
                binding: 2,
                buffer: &cumulative_buffer,
                read_only: true,
            },
            ComputeBinding::Storage {
                binding: 3,
                buffer: &instance_buffer,
                read_only: true,
            },
            ComputeBinding::Storage {
                binding: 4,
                buffer: destination,
                read_only: false,
            },
        ];
        if let Some((normal_buffer, _)) = normals {
            bindings.push(ComputeBinding::Storage {
                binding: 5,
                buffer: normal_buffer,
                read_only: false,
            });
        }
        // P3-R251：压缩档的 indirect 参数（必须带 `INDIRECT`，不能借用 arena 的存储槽）。
        // 初始 `[index_count, 0, 0, 0, 0]`；P3-R271 起存活数由主 kernel 的 `atomicAdd` 直接
        // 累在 `args[1]` 上，不再有单独的 counter 缓冲与收尾 dispatch。
        let compact = compact_index_count.map(|index_count| {
            if let Some(set) = cached.as_ref() {
                return set.args.clone();
            }
            let mut args = [0u32; 5];
            args[0] = index_count;
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("rustlua-batch-motion-compact-args"),
                    contents: bytemuck::cast_slice(&args),
                    usage: wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::INDIRECT
                        | wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::COPY_SRC,
                })
        });
        let workgroups = [(instances.len() as u32).div_ceil(WORKGROUP_SIZE), 1, 1];
        if let Some(args_buffer) = compact.as_ref() {
            bindings.push(ComputeBinding::Storage {
                binding: 6,
                buffer: args_buffer,
                read_only: false,
            });
        }
        let request = ComputeRequest {
            label: "rustlua-batch-motion",
            shader: SHADER,
            entry_point: match (normals.is_some(), compact.is_some()) {
                (true, true) => "main_normals_compact",
                (true, false) => "main_normals",
                (false, true) => "main_compact",
                (false, false) => "main",
            },
            bindings: &bindings,
            workgroups,
            // P3-R284：绑定组身份 = 持久输入集身份 + 实例 arena 代数，**且目标必须就是本帧
            // arena**。绑定 4/5 绑的是调用方给的缓冲：帧路径给的是整块 arena（矩阵格偏移走
            // params），外部对账门/回读每次新建目标缓冲——那条路缓存不得，否则第二帧写进上
            // 一帧那块缓冲、新目标读回全零（R270 门实测过这个形状）。
            bind_group_key: cached
                .as_ref()
                .filter(|_| destination_is_arena)
                .map(|set| set.bind_serial),
        };
        if cache.is_some() {
            self.dispatch_compute_deferred(&request)?;
        } else {
            self.dispatch_compute(&request)?;
        }
        if let Some(args_buffer) = compact {
            return Ok((instances.len() as u32, Some(args_buffer)));
        }
        Ok((instances.len() as u32, None))
    }

    /// 对账通道：自建目标缓冲、求值、读回 CPU（**仅 native**；wasm 返回
    /// [`ComputeError::ReadbackUnsupported`]）。没有可求值的东西时返回空表。
    ///
    /// 这是给"GPU 与 CPU 求的是不是同一套位姿"这条门用的，不是逐帧路径。
    ///
    /// # Errors
    ///
    /// 同 [`Self::bake_batch_motion`]，外加回读失败。
    pub fn bake_batch_motion_readback(
        &mut self,
        batch: &InstanceBatch3D,
        time: f32,
    ) -> Result<Vec<Mat4>, BatchMotionGpuError> {
        self.bake_batch_motion_readback_with_cull(batch, time, None)
    }

    /// P3-R250：带逐实例距离剔除的回读（`cull` 语义同
    /// [`Self::bake_batch_motion_with_normals_and_cull`]）。
    ///
    /// # Errors
    ///
    /// 同 [`Self::bake_batch_motion_readback`]，外加剔除参数非有限。
    pub fn bake_batch_motion_readback_with_cull(
        &mut self,
        batch: &InstanceBatch3D,
        time: f32,
        cull: Option<(Vec3, f32)>,
    ) -> Result<Vec<Mat4>, BatchMotionGpuError> {
        let Some(motion) = batch.motion.as_ref() else {
            return Ok(Vec::new());
        };
        if batch.instances.is_empty() || motion.arc().is_none() {
            return Ok(Vec::new());
        }
        let instance_count = batch.instances.len();
        let size = instance_count as u64 * BATCH_MOTION_MATRIX_BYTES;
        let destination = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-batch-motion-readback-target"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let written = self.bake_batch_motion_with_normals_and_cull(
            batch,
            time,
            &destination,
            0,
            None,
            1.0,
            cull,
        )?;
        if written == 0 {
            return Ok(Vec::new());
        }
        let bytes = self.read_storage_buffer(&destination, size)?;
        let mut matrices = Vec::with_capacity(written as usize);
        for chunk in bytes.chunks_exact(BATCH_MOTION_MATRIX_BYTES as usize) {
            let mut columns = [[0.0f32; 4]; 4];
            for (column, slot) in columns.iter_mut().enumerate() {
                for (row, value) in slot.iter_mut().enumerate() {
                    let start = (column * 4 + row) * 4;
                    *value = f32::from_le_bytes([
                        chunk[start],
                        chunk[start + 1],
                        chunk[start + 2],
                        chunk[start + 3],
                    ]);
                }
            }
            matrices.push(Mat4 { columns });
        }
        Ok(matrices)
    }
}
