//! 第一个真实 compute 消费者：GPU 粒子（M74-03 P2）。
//!
//! ## 为什么是粒子
//!
//! 任务是"能力接缝 + 一个真实消费者"：只建 `ComputePipeline` 不算完成。粒子满足三个条件：
//! ① 逐粒子状态更新在 CPU 上已是可测热点（`rustlua_ecs::particle`，10k+ 时每帧全量遍历）；
//! ② 语义简单到可以**两档对拍**（同一初始状态、同一积分公式，位置差 < 1e-4）；
//! ③ 没有 compute 的平台（WebGL2 / 微信）今天就有 CPU 路径，GPU 档可以只是"可选加速"。
//!
//! ## 边界（刻意不做的事）
//!
//! - **不接管发射器生命周期**：`rate`（连续发射）在 v1 里显式拒绝，只做一次 burst + 逐帧积分。
//!   连续发射需要环形容量与回收策略，属于 P3-R07 的完整版；这里先不假装支持。
//! - **不回读给 CPU 渲染**：顶点由 compute 直接写进带 `VERTEX` 用法的存储缓冲，渲染侧零回读。
//!   `read_gpu_particles` 只给验收/调试用（native 同步回读）。
//! - **不改变默认画面**：没有调用 `configure_gpu_particles` 时，本模块一个 pass 都不产生；
//!   原有 CPU 粒子/精灵路径逐字节不变。
//!
//! ## 数据流
//!
//! ```text
//! CPU 初始 burst（同一套 ParticleEmitter 语义）
//!   → storage buffer（ParticleState × capacity）
//!   → compute（积分 + 写 6 个 WorldSpriteVertex）
//!   → 同一块 buffer 作为 vertex buffer
//!   → 复用世界 Sprite 管线（白贴图 + alpha blend）画进 RenderPass::Particle
//! ```
//!
//! 初始状态由 CPU 生成而**不是**在 WGSL 里复刻随机数：这样两档对拍比较的是"积分是否一致"，
//! 而不是"两个 RNG 是否碰巧一致"。随机数一旦进 shader，对拍就失去意义。

use bytemuck::{Pod, Zeroable};
use rustlua_ecs::particle::{ParticleEmitter, ParticleSystem};
use rustlua_ecs::transform::Vec2;
use wgpu::util::DeviceExt;

use crate::host::frame::WorldSpriteVertex;

use super::world_sprite::WorldSpriteView;
use super::{
    ComputeBinding, ComputeError, ComputeRequest, RenderFrameMetrics, Renderer,
    apply_declared_viewport, record_set_pass, record_vertex_draw,
};

/// GPU 粒子 shader。
///
/// 顶点布局必须与 [`WorldSpriteVertex`] 逐字节一致（36 字节：pos vec3 + uv vec2 + color vec4）。
/// `ParticleState` 必须与 [`GpuParticleState`] 一致（48 字节）；两处都靠单测的长度断言钉住。
pub(super) const GPU_PARTICLE_SHADER: &str = r#"
struct ParticleState {
    position: vec2<f32>,
    velocity: vec2<f32>,
    life: f32,
    max_life: f32,
    size_start: f32,
    size_end: f32,
    rotation: f32,
    _pad0: f32,
    _pad1: vec2<f32>,
};

struct Params {
    dt: f32,
    drag: f32,
    rotation_speed: f32,
    _pad0: f32,
    gravity: vec2<f32>,
    _pad1: vec2<f32>,
    color_start: vec4<f32>,
    color_end: vec4<f32>,
};

struct Vertex {
    position: vec3<f32>,
    uv: vec2<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0) var<storage, read_write> states: array<ParticleState>;
// 顶点用**裸 f32 数组**存，而不是 `array<Vertex>`：WGSL 的 `vec3` 对齐是 16，`Vertex` 的
// 实际 stride 会变成 48 字节；渲染管线的 `WorldSpriteVertex` 是 repr(C) 的 36 字节。
// 两边 stride 不一致时顶点拉取会读到错位数据（而且 compute 的写入会越过绑定范围被丢弃），
// 症状是"draw 调了、屏幕全黑"。裸数组让布局由这里的索引算术唯一决定。
@group(0) @binding(1) var<storage, read_write> vertices: array<f32>;
@group(0) @binding(2) var<uniform> params: Params;

const QUAD: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>( 1.0,  1.0),
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 1.0,  1.0),
    vec2<f32>(-1.0,  1.0),
);

// 9 个 f32 = position.xyz + uv.xy + color.rgba = 36 字节。
const FLOATS_PER_VERTEX: u32 = 9u;
const VERTICES_PER_PARTICLE: u32 = 6u;

fn write_vertex(slot: u32, position: vec3<f32>, uv: vec2<f32>, color: vec4<f32>) {
    let base = slot * FLOATS_PER_VERTEX;
    vertices[base + 0u] = position.x;
    vertices[base + 1u] = position.y;
    vertices[base + 2u] = position.z;
    vertices[base + 3u] = uv.x;
    vertices[base + 4u] = uv.y;
    vertices[base + 5u] = color.r;
    vertices[base + 6u] = color.g;
    vertices[base + 7u] = color.b;
    vertices[base + 8u] = color.a;
}

fn emit_degenerate(index: u32) {
    for (var k = 0u; k < VERTICES_PER_PARTICLE; k = k + 1u) {
        write_vertex(
            index * VERTICES_PER_PARTICLE + k,
            vec3<f32>(0.0, 0.0, 0.0),
            vec2<f32>(0.0, 0.0),
            vec4<f32>(0.0, 0.0, 0.0, 0.0),
        );
    }
}

@compute @workgroup_size(64)
fn step(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index = gid.x;
    if (index >= arrayLength(&states)) {
        return;
    }
    var state = states[index];
    if (state.life <= 0.0) {
        // 死亡粒子保留槽位并输出退化四边形：draw 范围因此恒为 capacity*6，
        // 不需要每帧回读"还活着几个"，也不需要 indirect draw。
        emit_degenerate(index);
        return;
    }
    let dt = params.dt;
    state.velocity = state.velocity + params.gravity * dt;
    if (params.drag > 0.0) {
        let damp = max(1.0 - params.drag * dt, 0.0);
        state.velocity = state.velocity * damp;
    }
    state.position = state.position + state.velocity * dt;
    state.rotation = state.rotation + params.rotation_speed * dt;
    state.life = state.life - dt;
    states[index] = state;

    let progress = clamp(1.0 - state.life / state.max_life, 0.0, 1.0);
    let size = mix(state.size_start, state.size_end, progress);
    let color = mix(params.color_start, params.color_end, progress);
    let half = size * 0.5;
    let sin_rotation = sin(state.rotation);
    let cos_rotation = cos(state.rotation);
    for (var k = 0u; k < VERTICES_PER_PARTICLE; k = k + 1u) {
        let corner = QUAD[k];
        let rotated = vec2<f32>(
            (corner.x * cos_rotation - corner.y * sin_rotation) * half,
            (corner.x * sin_rotation + corner.y * cos_rotation) * half,
        );
        write_vertex(
            index * VERTICES_PER_PARTICLE + k,
            vec3<f32>(state.position + rotated, 0.0),
            corner * 0.5 + vec2<f32>(0.5, 0.5),
            color,
        );
    }
}
"#;

/// 一个粒子的 GPU 状态（与 WGSL `ParticleState` 逐字节对应，48 字节）。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct GpuParticleState {
    pub position: [f32; 2],
    pub velocity: [f32; 2],
    pub life: f32,
    pub max_life: f32,
    pub size_start: f32,
    pub size_end: f32,
    pub rotation: f32,
    _padding: f32,
    _padding2: [f32; 2],
}

impl GpuParticleState {
    fn from_cpu(particle: &rustlua_ecs::particle::Particle) -> Self {
        Self {
            position: [particle.position.x, particle.position.y],
            velocity: [particle.velocity.x, particle.velocity.y],
            life: particle.life,
            max_life: particle.max_life,
            size_start: particle.size_start,
            size_end: particle.size_end,
            rotation: particle.rotation,
            _padding: 0.0,
            _padding2: [0.0; 2],
        }
    }
}

/// 一次 GPU 粒子的配置。
///
/// 语义与 [`ParticleEmitter`] 一致；`capacity` 是显存里预留的槽位（≥ `emitter.count`）。
/// `rate > 0`（连续发射）在 v1 里会被**拒绝**而不是静默忽略。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuParticleConfig {
    /// burst 的世界坐标（2D）。
    pub origin: [f32; 2],
    /// 与 CPU 粒子系统共用同一份发射参数（单一口径）。
    pub emitter: ParticleEmitter,
    /// GPU 槽位上限；`emitter.count` 必须 ≤ 它。
    pub capacity: u32,
}

/// 配置成功后的读数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuParticleStatus {
    pub capacity: u32,
    pub spawned: u32,
    pub state_bytes: u64,
    pub vertex_bytes: u64,
    /// compute 后端短名（`vulkan`/`dx12`/…）。
    pub backend: &'static str,
    pub steps: u64,
}

/// 一次 step 的读数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuParticleStep {
    /// 累计 step 次数。
    pub steps: u64,
}

/// 调试回读的一条粒子状态（`life <= 0` = 已死亡但槽位仍在）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuParticleSnapshot {
    pub position: [f32; 2],
    pub velocity: [f32; 2],
    pub life: f32,
    pub max_life: f32,
    pub size_start: f32,
    pub size_end: f32,
    pub rotation: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GpuParticleParams {
    dt: f32,
    drag: f32,
    rotation_speed: f32,
    _padding: f32,
    gravity: [f32; 2],
    _padding2: [f32; 2],
    color_start: [f32; 4],
    color_end: [f32; 4],
}

/// 渲染器持有的 GPU 粒子系统（`None` = 没启用，一个 pass 都不产生）。
pub(super) struct GpuParticleSystem {
    emitter: ParticleEmitter,
    capacity: u32,
    spawned: u32,
    steps: u64,
    state_buffer: wgpu::Buffer,
    vertex_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    /// 绑定组缓存身份。`configure_gpu_particles` 会先清空 compute 缓存，因此固定 id
    /// 不会命到上一组缓冲（重配必须换 buffer，缓存必须一起失效）。
    bind_group_key: u64,
    state_bytes: u64,
    vertex_bytes: u64,
}

impl GpuParticleSystem {
    pub(super) const fn is_active(&self) -> bool {
        self.capacity > 0
    }

    pub(super) fn status(&self, backend: &'static str) -> GpuParticleStatus {
        GpuParticleStatus {
            capacity: self.capacity,
            spawned: self.spawned,
            state_bytes: self.state_bytes,
            vertex_bytes: self.vertex_bytes,
            backend,
            steps: self.steps,
        }
    }
}

const WORKGROUP_SIZE: u32 = 64;

impl Renderer {
    /// 配置 GPU 粒子：用 CPU 语义生成**同一份初始状态**，上传后由 compute 逐帧积分。
    ///
    /// 无 compute 时返回 [`ComputeError::Unsupported`]；宿主应继续走 CPU 粒子（默认档）。
    ///
    /// # Errors
    ///
    /// 能力缺失、`emitter.rate > 0`、容量/绑定超限、参数非有限。
    pub fn configure_gpu_particles(
        &mut self,
        config: GpuParticleConfig,
    ) -> Result<GpuParticleStatus, ComputeError> {
        if !self.compute_enabled() {
            return Err(self.compute_not_available());
        }
        config
            .emitter
            .validate()
            .map_err(|error| ComputeError::InvalidConfig(error.to_owned()))?;
        if config.emitter.rate > 0.0 {
            return Err(ComputeError::InvalidConfig(
                "GPU 粒子 v1 只支持 burst（rate > 0 的连续发射尚未实现，拒绝而不是静默忽略）"
                    .to_owned(),
            ));
        }
        if config.emitter.count == 0 {
            return Err(ComputeError::InvalidConfig(
                "emitter.count = 0：没有粒子可更新".to_owned(),
            ));
        }
        let count = u32::try_from(config.emitter.count).map_err(|_| {
            ComputeError::InvalidConfig(format!(
                "emitter.count={} 超过 u32 槽位表示范围",
                config.emitter.count
            ))
        })?;
        if config.capacity == 0 || config.capacity < count {
            return Err(ComputeError::InvalidConfig(format!(
                "capacity={} 必须 ≥ emitter.count={} 且 > 0",
                config.capacity, config.emitter.count
            )));
        }
        if self.capabilities.max_compute_workgroup_size[0] < WORKGROUP_SIZE
            || self.capabilities.max_compute_invocations_per_workgroup < WORKGROUP_SIZE
        {
            return Err(ComputeError::InvalidConfig(format!(
                "设备 workgroup 上限 {}x/{} 调用不足以容纳 {WORKGROUP_SIZE} 线程",
                self.capabilities.max_compute_workgroup_size[0],
                self.capabilities.max_compute_invocations_per_workgroup
            )));
        }
        let workgroups = config.capacity.div_ceil(WORKGROUP_SIZE);
        if workgroups > self.capabilities.max_compute_workgroups_per_dimension {
            return Err(ComputeError::InvalidConfig(format!(
                "capacity={} 需要 {workgroups} 个 workgroup，超过设备上限 {}",
                config.capacity, self.capabilities.max_compute_workgroups_per_dimension
            )));
        }
        let state_bytes =
            u64::from(config.capacity) * std::mem::size_of::<GpuParticleState>() as u64;
        let vertex_bytes =
            u64::from(config.capacity) * 6 * std::mem::size_of::<WorldSpriteVertex>() as u64;
        if state_bytes > self.capabilities.max_storage_buffer_binding_size {
            return Err(ComputeError::BindingTooLarge {
                binding: 0,
                requested: state_bytes,
                limit: self.capabilities.max_storage_buffer_binding_size,
            });
        }
        if vertex_bytes > self.capabilities.max_storage_buffer_binding_size {
            return Err(ComputeError::BindingTooLarge {
                binding: 1,
                requested: vertex_bytes,
                limit: self.capabilities.max_storage_buffer_binding_size,
            });
        }

        // CPU 语义生成初始状态：随机数留在 CPU，GPU 只负责积分。
        let mut cpu = ParticleSystem::new();
        cpu.burst_with(
            Vec2::new(config.origin[0], config.origin[1]),
            &config.emitter,
        );
        let spawned = cpu.len() as u32;
        if spawned == 0 {
            return Err(ComputeError::InvalidConfig(
                "CPU 初始 burst 没有生成任何粒子".to_owned(),
            ));
        }
        if spawned > config.capacity {
            return Err(ComputeError::InvalidConfig(format!(
                "CPU burst 生成了 {spawned} 个粒子，超过 capacity={}",
                config.capacity
            )));
        }
        let mut states = vec![GpuParticleState::zeroed(); config.capacity as usize];
        for (index, particle) in cpu.particles().iter().enumerate() {
            states[index] = GpuParticleState::from_cpu(particle);
        }

        let state_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-gpu-particles-state"),
                contents: bytemuck::cast_slice(&states),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            });
        let vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-gpu-particles-vertices"),
            size: vertex_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        let params = Self::gpu_particle_params(&config.emitter, 0.0);
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-gpu-particles-params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        // 重新配置：旧缓冲已交出，旧绑定组不能再命中。
        self.compute_state.clear();
        self.gpu_particles = Some(GpuParticleSystem {
            emitter: config.emitter,
            capacity: config.capacity,
            spawned,
            steps: 0,
            state_buffer,
            vertex_buffer,
            params_buffer,
            bind_group_key: 1,
            state_bytes,
            vertex_bytes,
        });
        let status = self
            .gpu_particles
            .as_ref()
            .expect("just set")
            .status(self.capabilities.backend_name());
        tracing::info!(
            target: "render",
            capacity = status.capacity,
            spawned = status.spawned,
            state_bytes = status.state_bytes,
            vertex_bytes = status.vertex_bytes,
            backend = status.backend,
            "[render] GPU 粒子已配置（compute 档；CPU 档仍是默认与回滚）"
        );
        Ok(status)
    }

    /// 关闭 GPU 粒子并释放其缓冲。之后 `RenderPass::Particle` 不再出现在计划里。
    pub fn clear_gpu_particles(&mut self) {
        if self.gpu_particles.take().is_some() {
            self.compute_state.clear();
            tracing::info!(target: "render", "[render] GPU 粒子已关闭");
        }
    }

    /// 推进一帧。`dt <= 0`/非有限值与 CPU `ParticleSystem::step` 一致：**不改状态**。
    ///
    /// # Errors
    ///
    /// 没配置（`NotConfigured`）、无 compute/被关闭（`Unsupported`）、dispatch 校验失败。
    pub fn step_gpu_particles(
        &mut self,
        delta_seconds: f32,
    ) -> Result<GpuParticleStep, ComputeError> {
        if !self.compute_enabled() {
            return Err(self.compute_not_available());
        }
        let Some(system) = self.gpu_particles.as_ref() else {
            return Err(ComputeError::NotConfigured);
        };
        if !delta_seconds.is_finite() || delta_seconds <= 0.0 {
            return Ok(GpuParticleStep {
                steps: system.steps,
            });
        }
        let params = Self::gpu_particle_params(&system.emitter, delta_seconds);
        self.queue
            .write_buffer(&system.params_buffer, 0, bytemuck::bytes_of(&params));
        let workgroups = system.capacity.div_ceil(WORKGROUP_SIZE);
        // `dispatch_compute` 要 `&mut self`，而 `bindings` 借用 `system` 的字段。wgpu 的
        // 缓冲句柄是 Clone（内部引用计数），先 clone 成局部句柄即可结束对 `self` 的借用。
        let state_buffer = system.state_buffer.clone();
        let vertex_buffer = system.vertex_buffer.clone();
        let params_buffer = system.params_buffer.clone();
        let bind_group_key = system.bind_group_key;
        let request = ComputeRequest {
            label: "rustlua-gpu-particles-step",
            shader: GPU_PARTICLE_SHADER,
            entry_point: "step",
            bindings: &[
                ComputeBinding::Storage {
                    binding: 0,
                    buffer: &state_buffer,
                    read_only: false,
                },
                ComputeBinding::Storage {
                    binding: 1,
                    buffer: &vertex_buffer,
                    read_only: false,
                },
                ComputeBinding::Uniform {
                    binding: 2,
                    buffer: &params_buffer,
                },
            ],
            workgroups: [workgroups, 1, 1],
            bind_group_key: Some(bind_group_key),
        };
        self.dispatch_compute(&request)?;
        let system = self.gpu_particles.as_mut().expect("checked above");
        system.steps += 1;
        Ok(GpuParticleStep {
            steps: system.steps,
        })
    }

    /// 调试回读：把槽位状态拷回 CPU（仅 native；wasm 返回 `ReadbackUnsupported`）。
    ///
    /// # Errors
    ///
    /// 没配置、无 compute/被关闭、平台不支持、映射失败。
    pub fn read_gpu_particles(&self) -> Result<Vec<GpuParticleSnapshot>, ComputeError> {
        if !self.compute_enabled() {
            return Err(self.compute_not_available());
        }
        let Some(system) = self.gpu_particles.as_ref() else {
            return Err(ComputeError::NotConfigured);
        };
        let bytes = self.read_storage_buffer(&system.state_buffer, system.state_bytes)?;
        let mut snapshots = Vec::with_capacity(system.spawned as usize);
        for chunk in bytes.chunks_exact(std::mem::size_of::<GpuParticleState>()) {
            let state = bytemuck::pod_read_unaligned::<GpuParticleState>(chunk);
            if snapshots.len() >= system.spawned as usize {
                break;
            }
            snapshots.push(GpuParticleSnapshot {
                position: state.position,
                velocity: state.velocity,
                life: state.life,
                max_life: state.max_life,
                size_start: state.size_start,
                size_end: state.size_end,
                rotation: state.rotation,
            });
        }
        Ok(snapshots)
    }

    /// 已配置的 GPU 粒子读数（诊断/验收用）。
    #[must_use]
    pub fn gpu_particle_status(&self) -> Option<GpuParticleStatus> {
        self.gpu_particles
            .as_ref()
            .map(|system| system.status(self.capabilities.backend_name()))
    }

    fn gpu_particle_params(emitter: &ParticleEmitter, dt: f32) -> GpuParticleParams {
        GpuParticleParams {
            dt,
            drag: emitter.drag,
            rotation_speed: emitter.rotation_speed,
            _padding: 0.0,
            gravity: [emitter.gravity.x, emitter.gravity.y],
            _padding2: [0.0; 2],
            color_start: emitter.color_start,
            color_end: emitter.color_end,
        }
    }

    /// 画本帧的 GPU 粒子。返回是否真的产生了一次 draw。
    ///
    /// 复用世界 Sprite 管线（group0 = 相机，group1 = 贴图）：粒子是半透明段，测深度不写深度。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_gpu_particles(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        target_size: (u32, u32),
        world: WorldSpriteView,
        clear_depth: bool,
        metrics: &mut RenderFrameMetrics,
    ) -> bool {
        let Some(white) = self.ensure_white() else {
            // 白贴图都建不出来时，宁可记一条 dropped 也不静默空转。
            metrics.dropped_draws += 1;
            tracing::warn!(
                target: "render",
                "[render] GPU 粒子丢弃：白贴图 bind group 不可用"
            );
            return false;
        };
        let Some(system) = self.gpu_particles.as_ref() else {
            return false;
        };
        let vertex_count = system.capacity * 6;
        if vertex_count == 0 {
            return false;
        }
        let depth_view = if world.shared_depth {
            &self.depth_view
        } else {
            &self.world_sprite_depth_view
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rustlua-gpu-particles-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: if clear_depth {
                        wgpu::LoadOp::Clear(1.0)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.world_sprite_pipeline);
        record_set_pass(metrics);
        pass.set_bind_group(0, &self.world_sprite_camera_bind_group, &[]);
        pass.set_bind_group(1, &white, &[]);
        pass.set_vertex_buffer(0, system.vertex_buffer.slice(..));
        apply_declared_viewport(&mut pass, world.viewport, target_size);
        pass.draw(0..vertex_count, 0..1);
        record_vertex_draw(metrics, vertex_count);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 顶点/状态布局与 WGSL 对齐：这两条长度一旦漂移，画面上是"粒子错位/颜色乱"，
    /// 而 wgpu 的校验只会在管线创建时给一条难读的 stride 错误。
    #[test]
    fn gpu_layouts_match_wgsl_sizes() {
        assert_eq!(std::mem::size_of::<GpuParticleState>(), 48);
        assert_eq!(std::mem::size_of::<WorldSpriteVertex>(), 36);
        assert_eq!(std::mem::size_of::<GpuParticleParams>(), 64);
    }
}
