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
//!    indirect 参数携带真实实例数——顺序不稳定，画面等价。
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

use bytemuck::{Pod, Zeroable};
use rustlua_math::math3d::{Mat4, Vec3};
use rustlua_render::instance_batch::{ForwardAxis, InstanceBatch3D};
use wgpu::util::DeviceExt;

use super::{ComputeBinding, ComputeError, ComputeRequest, Renderer};

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
/// （+8 字节填充）；P3-R255 的视锥六面落在 96..192。
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
    _pad0: u32,
    // P3-R250：剔除球心（相机世界位置）+ 半径；半径 <= 0 = 关闭。
    cull_origin: [f32; 3],
    cull_distance: f32,
    /// P3-R266：远程密度档的半径（米）；`<= 0` 或 `thin_stride < 2` = 关闭。
    thin_distance: f32,
    /// P3-R266：远区保留 `index % thin_stride == 0` 的实例（`1` = 关）。
    thin_stride: u32,
    _pad1: [f32; 2],
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
            )?
            .1)
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

        // 路径节点与累计弧长：与 CPU `matrices_at` 用的是同一份去重/累计口径。
        let nodes = arc.nodes(&motion.path);
        let mut node_data: Vec<f32> = Vec::with_capacity(nodes.len() * 4);
        for node in &nodes {
            node_data.extend_from_slice(&[node[0], node[1], node[2], 0.0]);
        }
        let cumulative = arc.cumulative();
        // 逐实例参数：offset / speed / flags / 静态旋转 / 缩放。
        let mut instance_data: Vec<f32> = Vec::with_capacity(instances.len() * 12);
        for instance in instances {
            let params = instance.motion.unwrap_or_default();
            let mut flags = 0u32;
            if params.align {
                flags |= 1;
            }
            if params.forward == ForwardAxis::Z {
                flags |= 2;
            }
            instance_data.extend_from_slice(&[
                params.offset,
                params.speed,
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
            _pad0: 0,
            // P3-R250：None 或半径 <= 0 ⇒ 0，shader 侧判定 `> 0.0` 才剔除。
            // P3-R266：远程密度档与距离剔除共用同一个球心（相机世界位置）——thin 单独开时
            // 也要把这个原点带过去，否则 shader 会拿 [0,0,0] 当相机量距离。
            cull_origin: cull
                .map(|(origin, _)| origin)
                .or(thin.map(|(origin, _, _)| origin))
                .map_or([0.0; 3], |origin| [origin.x, origin.y, origin.z]),
            cull_distance: cull.map_or(0.0, |(_, distance)| distance.max(0.0)),
            thin_distance: thin.map_or(0.0, |(_, distance, _)| distance.max(0.0)),
            thin_stride: thin.map_or(1, |(_, _, stride)| stride.max(1)),
            _pad1: [0.0; 2],
            thin_distance2: thin_far.map_or(0.0, |(_, distance, _)| distance.max(0.0)),
            thin_stride2: thin_far.map_or(1, |(_, _, stride)| stride.max(1)),
            _pad2: [0.0; 2],
            frustum: frustum.unwrap_or([[0.0; 4]; 6]),
        };

        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-batch-motion-params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let node_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-batch-motion-nodes"),
                contents: bytemuck::cast_slice(&node_data),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let cumulative_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-batch-motion-cumulative"),
                contents: bytemuck::cast_slice(cumulative),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let instance_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-batch-motion-instances"),
                contents: bytemuck::cast_slice(&instance_data),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });

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
        // P3-R251：压缩档的计数器与 indirect 参数。两者每帧新建（参数缓冲必须带
        // `INDIRECT`，不能借用 arena 的存储槽）；计数由原子加产出，参数初始
        // `[index_count, 0, 0, 0, 0]`，收尾 kernel 只回填 `instance_count`。
        let compact = compact_index_count.map(|index_count| {
            let counter = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-batch-motion-compact-counter"),
                size: 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&counter, 0, &0u32.to_le_bytes());
            let mut args = [0u32; 5];
            args[0] = index_count;
            let args_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("rustlua-batch-motion-compact-args"),
                    contents: bytemuck::cast_slice(&args),
                    usage: wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::INDIRECT
                        | wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::COPY_SRC,
                });
            (counter, args_buffer)
        });
        let workgroups = [(instances.len() as u32).div_ceil(WORKGROUP_SIZE), 1, 1];
        if let Some((counter, args_buffer)) = compact.as_ref() {
            bindings.push(ComputeBinding::Storage {
                binding: 6,
                buffer: counter,
                read_only: false,
            });
            bindings.push(ComputeBinding::Storage {
                binding: 7,
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
            // 缓冲每次调用都新建（节点/参数随批次内容变），绑定组按缓冲指针缓存会绑错——
            // 与 compute 接缝文档的第 2 条硬约束一致：没有稳定身份就不缓存。
            bind_group_key: None,
        };
        self.dispatch_compute(&request)?;
        if let Some((counter, args_buffer)) = compact {
            let finalize_bindings = [
                ComputeBinding::Storage {
                    binding: 6,
                    buffer: &counter,
                    read_only: false,
                },
                ComputeBinding::Storage {
                    binding: 7,
                    buffer: &args_buffer,
                    read_only: false,
                },
            ];
            let finalize = ComputeRequest {
                label: "rustlua-batch-motion-compact-finalize",
                shader: SHADER,
                entry_point: "finalize_compact",
                bindings: &finalize_bindings,
                workgroups: [1, 1, 1],
                bind_group_key: None,
            };
            self.dispatch_compute(&finalize)?;
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
