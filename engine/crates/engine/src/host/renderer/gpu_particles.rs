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
//!   （场景侧 `ParticleEmitter` 只能按 `rate` 发射，所以 `mode=gpu` 的场景发射器现在恒定回退
//!   `continuous_rate`；进 GPU 的是**一次性 burst**——Lua `particles.burst(..., {mode='gpu'})`。）
//! - **不回读给 CPU 渲染**：compute 只写**状态**，几何在顶点着色器里逐实例展开，渲染侧零回读。
//!   `read_gpu_particles` 只给验收/调试用（native 同步回读）。
//! - **不改变默认画面**：没有调用 `configure_gpu_particles` 时，本模块一个 pass 都不产生；
//!   原有 CPU 粒子/精灵路径逐字节不变。
//!
//! ## 数据流
//!
//! ```text
//! CPU 初始 burst（同一套 ParticleEmitter 语义）
//!   → storage buffer（ParticleState × capacity，48 B/槽）
//!   → compute（只积分状态；死槽一位不写）
//!   → 同一块 buffer 作为**实例流**（group2 只读存储）
//!   → instanced 管线：4 顶点 triangle strip，无顶点缓冲，复用世界 Sprite 的相机与白贴图
//!   → 画进 RenderPass::Particle（半透明：测深度不写深度）
//! ```
//!
//! 初始状态由 CPU 生成而**不是**在 WGSL 里复刻随机数：这样两档对拍比较的是"积分是否一致"，
//! 而不是"两个 RNG 是否碰巧一致"。随机数一旦进 shader，对拍就失去意义。
//!
//! ## 为什么 R07-3 把顶点展开搬进 shader
//!
//! 旧路径的 compute 每帧为每个粒子写 6 个 `WorldSpriteVertex`（216 B），死粒子也不例外——
//! 因为它无法通知绘制侧"这一批可以跳过"。instanced 之后每帧每粒子最多写 48 B（状态），
//! 剔除改由顶点着色器按 `life` 判定，于是"容量 1 万、活着 3 百"的场景不再为 97% 的死槽付写带宽。

use bytemuck::{Pod, Zeroable};
use rustlua_ecs::particle::{ParticleEmitter, ParticleSystem};
use rustlua_ecs::transform::Vec2;
use wgpu::util::DeviceExt;

use super::construct::build_gpu_particle_pipeline;
use super::world_sprite::WorldSpriteView;
use super::{
    ComputeBinding, ComputeError, ComputeRequest, DepthPass, RenderFrameMetrics, Renderer,
    apply_declared_viewport, record_instanced_quad_draw, record_set_pass,
};

/// GPU 粒子的共享声明段。
///
/// 必须是**宏**而不是常量：`concat!` 只接受字面量，而 compute 段与 draw 段是两个
/// `ShaderModule`（绑定组布局完全不同：compute 走 group0 的 storage+uniform，draw 复用
/// 世界 Sprite 的 group0=相机 / group1=贴图，粒子资源进 group2）。两个模块各自 `include`
/// 同一段文本，`ParticleState` 的字段布局因此不可能分叉——它们读同一份缓冲。
///
/// `ParticleState` 必须与 [`GpuParticleState`] 逐字节一致（48 字节），`Params` 必须与
/// [`GpuParticleParams`] 一致（64 字节）；两处都靠 [`tests::gpu_layouts_match_wgsl_sizes`] 钉住。
macro_rules! particle_common {
    () => {
        r#"struct ParticleState {
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

"#
    };
}

/// compute 段：每帧只积分**状态**，不再展开顶点。
///
/// P3-R07-3 之前这里还写 6×36 字节的顶点（216 B/粒子），几何由 CPU 侧的 quad 角点乘旋转、
/// 缩放后逐顶点落盘；现在那三步搬进顶点着色器，实例流就是状态缓冲本身。
pub(super) const GPU_PARTICLE_SHADER: &str = concat!(
    particle_common!(),
    r#"
@group(0) @binding(0) var<storage, read_write> states: array<ParticleState>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(64)
fn step(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index = gid.x;
    if (index >= arrayLength(&states)) {
        return;
    }
    var state = states[index];
    if (state.life <= 0.0) {
        // 死亡粒子一个字节都不写：绘制侧按 `instance_index` 读到 `life <= 0` 自己剔除，
        // 于是 draw 范围恒为 capacity 个实例，仍然不需要每帧回读"还活着几个"、也不需要
        // indirect draw。旧实现为此给每个死粒子写 6 个退化顶点（216 B），那是 R07-3
        // 要消掉的写带宽。
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
}
"#
);

/// draw 段：4 顶点 triangle strip 的**实例化** quad。
///
/// `group0`/`group1` 与世界 Sprite 管线逐字段相同，所以相机与白贴图那两条现成绑定组直接复用；
/// 粒子自己的两份资源在 `group2`。顶点着色器读状态缓冲用的是 `storage, read`——WebGPU 允许
/// 顶点阶段只读存储缓冲，而这条路径的前置条件本来就包含"设备有 compute"，WebGL2 到不了这里。
const GPU_PARTICLE_DRAW_SHADER: &str = concat!(
    particle_common!(),
    r#"
struct Camera { view_projection: mat4x4<f32> };

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(2) @binding(0) var<uniform> params: Params;
@group(2) @binding(1) var<storage, read> states: array<ParticleState>;

// triangle strip 的四角：与旧实现的六个（两条三角形）同一组坐标、同一 winding。
const CORNERS: array<vec2<f32>, 4> = array<vec2<f32>, 4>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>( 1.0,  1.0),
    vec2<f32>(-1.0,  1.0),
);

@vertex fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    var out: VertexOutput;
    let state = states[instance_index];
    if (state.life <= 0.0) {
        // 四角全部落到裁剪体外（x/y > w）：这个实例面积为零，一个片元都不产生。
        out.position = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        out.uv = vec2<f32>(0.0, 0.0);
        out.color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        return out;
    }
    let progress = clamp(1.0 - state.life / state.max_life, 0.0, 1.0);
    let size = mix(state.size_start, state.size_end, progress);
    let half = size * 0.5;
    let sin_rotation = sin(state.rotation);
    let cos_rotation = cos(state.rotation);
    let corner = CORNERS[vertex_index % 4u];
    let rotated = vec2<f32>(
        (corner.x * cos_rotation - corner.y * sin_rotation) * half,
        (corner.x * sin_rotation + corner.y * cos_rotation) * half,
    );
    // 透视视图（3D）下做 billboard：把 quad 的两个轴换成相机的世界右/上向量，粒子始终正对
    // 相机。右/上直接取自 view-projection 的第 0/1 行——透视投影只有对角缩放，归一化之后
    // 就是视图矩阵的相机基（不另传 uniform，也就不会与相机矩阵不同步）。
    // 正交视图（2D）保持历史行为：quad 在世界 XY 平面内展开，不随相机旋转——否则带旋转的
    // 2D 相机会让既有粒子画面整体翻转。
    // 判别式看**齐次 w 行的线性部分**：正交投影的 w 恒为 1（前三个分量为 0），透视投影的 w
    // 是沿相机前向的距离。不能用 `[3][3]`：view-projection 的 w 行带相机平移，透视下也不是 0。
    let perspective = abs(camera.view_projection[0][3])
        + abs(camera.view_projection[1][3])
        + abs(camera.view_projection[2][3]) > 0.5;
    var world = vec3<f32>(state.position, 0.0);
    if (perspective) {
        let right = normalize(vec3<f32>(
            camera.view_projection[0][0],
            camera.view_projection[1][0],
            camera.view_projection[2][0],
        ));
        let up = normalize(vec3<f32>(
            camera.view_projection[0][1],
            camera.view_projection[1][1],
            camera.view_projection[2][1],
        ));
        world = world + right * rotated.x + up * rotated.y;
    } else {
        world = world + vec3<f32>(rotated, 0.0);
    }
    out.position = camera.view_projection * vec4<f32>(world, 1.0);
    out.uv = corner * 0.5 + vec2<f32>(0.5, 0.5);
    out.color = mix(params.color_start, params.color_end, progress);
    return out;
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv) * in.color;
}
"#
);

/// 实例化 quad 的顶点数（triangle strip，无需索引缓冲、也无需顶点缓冲）。
pub(super) const QUAD_VERTEX_COUNT: u32 = 4;

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
///
/// P3-R07-3 起没有 `vertex_bytes` 这一项了：instanced 路径的**实例流就是状态缓冲**，
/// 两者是同一块显存，再报一个"顶点字节"只会读成两份并不存在的开销。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuParticleStatus {
    pub capacity: u32,
    pub spawned: u32,
    /// 状态/实例缓冲的字节数（`capacity × size_of::<GpuParticleState>()`）。
    pub state_bytes: u64,
    /// compute 后端短名（`vulkan`/`dx12`/…）。
    pub backend: &'static str,
    pub steps: u64,
}

/// 一次 step 的读数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuParticleStep {
    /// 累计 step 次数（池里最老的那个系统）。
    pub steps: u64,
    /// 本步之后仍在池里的系统数。
    pub systems: usize,
    /// 本步因寿命走完（或 dispatch 失败）而被回收的系统数。
    pub recycled: usize,
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

/// 渲染器持有的一组 GPU 粒子系统（空 = 没启用，一个 pass 都不产生）。
///
/// P3-R07-2 起这是**池**而不是单槽：一次 burst = 一个系统。之所以不能留单槽——
/// [`Renderer::configure_gpu_particles`] 的语义是"重配"（旧缓冲当场交出、旧粒子立刻消失），
/// 同一帧两个 `mode=gpu` 发射器爆发时后者会把前者挤掉，玩家看到的是上一批粒子凭空不见。
/// 那是**丢粒子**，而本模块的契约是"回退不丢粒子"。
///
/// 回收按寿命而不靠读回：CPU 侧 `max_life = life × 0.5..=1.0`（见
/// `rustlua_ecs::particle::ParticleSystem::burst_with`）恒 ≤ `emitter.life`，因此
/// `elapsed > emitter.life` 之后这个系统里不可能还有活粒子，直接释放缓冲即可。
pub(super) struct GpuParticleSystem {
    emitter: ParticleEmitter,
    capacity: u32,
    spawned: u32,
    steps: u64,
    /// 自 burst 起累计的模拟时间（秒）—— [`Self::is_expired`] 的判据。
    elapsed: f32,
    state_buffer: wgpu::Buffer,
    /// draw 侧的 group2：`params` uniform + `states`（只读存储）。实例流就是
    /// [`Self::state_buffer`] 这一块，没有第二个缓冲。
    draw_bind_group: wgpu::BindGroup,
    params_buffer: wgpu::Buffer,
    /// 绑定组缓存身份（每个系统一个递增 id）。回收时按它清掉对应的绑定组，
    /// 否则"每帧爆发一次"的池会把绑定组缓存无限撑大。
    bind_group_key: u64,
    state_bytes: u64,
}

impl GpuParticleSystem {
    pub(super) const fn is_active(&self) -> bool {
        self.capacity > 0
    }

    /// 寿命是否已走完（池回收判据）。
    pub(super) const fn is_expired(&self) -> bool {
        self.elapsed > self.emitter.life
    }

    pub(super) fn status(&self, backend: &'static str) -> GpuParticleStatus {
        GpuParticleStatus {
            capacity: self.capacity,
            spawned: self.spawned,
            state_bytes: self.state_bytes,
            backend,
            steps: self.steps,
        }
    }
}

const WORKGROUP_SIZE: u32 = 64;

/// 同时在池里的 GPU 粒子系统上限（P3-R07-2）。
///
/// 到顶之后新 burst 被**拒绝**（调用方回退 CPU），不是排队、更不是挤掉旧系统。
pub const MAX_GPU_PARTICLE_SYSTEMS: usize = 16;

/// GPU 粒子的总槽位预算（跨所有系统）。
///
/// instanced 之后每槽只有一份状态 = 48 B ⇒ 65_536 槽约 3.1 MB（R07-3 之前是 264 B/槽
/// ≈ 17 MB，因为每槽还要展开 6 个顶点）。留着 65_536 这个数字不是为了省显存，而是它同时
/// 卡住两条设备上限：单绑定 128 MiB 远远够用，而 `capacity` 直接决定 compute 的 workgroup
/// 数与每帧的实例数。R07-4 的 50k 单次爆发占 2.4 MB，离这条线还很远。
pub const MAX_GPU_PARTICLE_SLOTS: u32 = 65_536;

impl Renderer {
    /// **整池重配** GPU 粒子：清空池，用 CPU 语义生成**同一份初始状态**上传，之后由 compute 逐帧积分。
    ///
    /// 无 compute 时返回 [`ComputeError::Unsupported`]；宿主应继续走 CPU 粒子（默认档）。
    ///
    /// 语义是"替换"，因此旧粒子当场消失。多个发射器同帧爆发（场景/脚本派发通道）要用
    /// [`Self::push_gpu_particle_burst`]，别在这里连续调用。
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
        let system = self.build_gpu_particle_system(config)?;
        let status = system.status(self.capabilities.backend_name());
        // 重新配置：旧缓冲已交出，旧绑定组不能再命中。
        self.compute_state.clear();
        self.gpu_particles = vec![system];
        tracing::info!(
            target: "render",
            capacity = status.capacity,
            spawned = status.spawned,
            state_bytes = status.state_bytes,
            backend = status.backend,
            "[render] GPU 粒子已配置（compute 档；CPU 档仍是默认与回滚）"
        );
        Ok(status)
    }

    /// P3-R07-2：把一次 burst **追加**进 GPU 粒子池（不动池里已有的系统）。
    ///
    /// 派发通道用它：一个 `mode=gpu` 的 burst = 一个系统。为什么不合并成一个系统——
    /// gravity/drag/rotation_speed/颜色曲线在 uniform 里是**按系统**的，把两个发射器的粒子
    /// 塞进同一系统等于用 A 的重力模拟 B。
    ///
    /// # Errors
    ///
    /// 与 [`Self::configure_gpu_particles`] 同一套校验，另加两条池预算：系统数达到
    /// [`MAX_GPU_PARTICLE_SYSTEMS`]、或总槽位超过 [`MAX_GPU_PARTICLE_SLOTS`] 时返回
    /// [`ComputeError::InvalidConfig`]。调用方（`RuntimeScene` 的派发通道）据此把这一批
    /// 退回 CPU 池——**拒绝而不是丢粒子**。
    pub fn push_gpu_particle_burst(
        &mut self,
        config: GpuParticleConfig,
    ) -> Result<GpuParticleStatus, ComputeError> {
        if !self.compute_enabled() {
            return Err(self.compute_not_available());
        }
        if self.gpu_particles.len() >= MAX_GPU_PARTICLE_SYSTEMS {
            return Err(ComputeError::InvalidConfig(format!(
                "GPU 粒子池已满（{} 个系统，上限 {MAX_GPU_PARTICLE_SYSTEMS}）",
                self.gpu_particles.len()
            )));
        }
        let used = self.gpu_particle_slots_used();
        let total = used.saturating_add(config.capacity);
        if total > MAX_GPU_PARTICLE_SLOTS {
            return Err(ComputeError::InvalidConfig(format!(
                "GPU 粒子槽位预算不足：已用 {used} + 本次 {} = {total}，超过上限 {MAX_GPU_PARTICLE_SLOTS}",
                config.capacity
            )));
        }
        let system = self.build_gpu_particle_system(config)?;
        let status = system.status(self.capabilities.backend_name());
        self.gpu_particles.push(system);
        Ok(status)
    }

    /// 池里已占用的槽位数（显存预算判定用；饱和加，绝不因为溢出而放行）。
    fn gpu_particle_slots_used(&self) -> u32 {
        self.gpu_particles
            .iter()
            .fold(0u32, |sum, system| sum.saturating_add(system.capacity))
    }

    /// 每个系统一个递增的绑定组身份；回收时按它精确清掉缓存条目。
    fn next_gpu_particle_key(&mut self) -> u64 {
        self.gpu_particle_generation = self.gpu_particle_generation.wrapping_add(1);
        self.gpu_particle_generation
    }

    /// 校验参数、按 CPU 语义生成初始状态并建好缓冲——**不**放进池。
    ///
    /// `configure_gpu_particles` 与 `push_gpu_particle_burst` 共用这一份判定，
    /// 两条入口对同一个 emitter 必须给出同一个"能/不能"，否则回退原因会随调用方变。
    fn build_gpu_particle_system(
        &mut self,
        config: GpuParticleConfig,
    ) -> Result<GpuParticleSystem, ComputeError> {
        config
            .emitter
            .validate()
            .map_err(|error| ComputeError::InvalidConfig(error.to_owned()))?;
        if self.gpu_particle_draw_layout.is_none() {
            return Err(ComputeError::Unsupported {
                reason: "设备没有存储缓冲（max_storage_buffers_per_shader_stage=0）：\
                         GPU 粒子 draw 布局未创建，走 CPU 粒子"
                    .to_owned(),
            });
        }
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
        if state_bytes > self.capabilities.max_storage_buffer_binding_size {
            return Err(ComputeError::BindingTooLarge {
                binding: 0,
                requested: state_bytes,
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
        let params = Self::gpu_particle_params(&config.emitter, 0.0);
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-gpu-particles-params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        // draw 侧的 group2。绑定的是**同一块**状态缓冲（只读）与这份参数：instanced 之后
        // 实例流就是状态本身，所以一个系统只需要一条绑定组，也不需要顶点缓冲。
        let draw_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-gpu-particles-draw"),
            layout: self
                .gpu_particle_draw_layout
                .as_ref()
                .expect("build_gpu_particle_system 入口已拒绝无布局的设备"),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: state_buffer.as_entire_binding(),
                },
            ],
        });

        Ok(GpuParticleSystem {
            emitter: config.emitter,
            capacity: config.capacity,
            spawned,
            steps: 0,
            elapsed: 0.0,
            state_buffer,
            draw_bind_group,
            params_buffer,
            bind_group_key: self.next_gpu_particle_key(),
            state_bytes,
        })
    }

    /// 关闭 GPU 粒子并释放池里全部系统的缓冲。之后 `RenderPass::Particle` 不再出现在计划里。
    pub fn clear_gpu_particles(&mut self) {
        if !self.gpu_particles.is_empty() {
            self.gpu_particles.clear();
            self.compute_state.clear();
            tracing::info!(target: "render", "[render] GPU 粒子已关闭");
        }
    }

    /// 推进池里**所有**系统一帧。`dt <= 0`/非有限值与 CPU `ParticleSystem::step` 一致：**不改状态**。
    ///
    /// 每帧末尾回收寿命走完的系统（缓冲释放 + 绑定组缓存条目按 id 精确剔除）。回收判据是
    /// `elapsed > emitter.life`，而 CPU 侧任何粒子的 `max_life` 都 ≤ `emitter.life`，因此
    /// 这一步**不需要读回**——GPU 粒子的"什么时候全死了"由发射参数就能推出上界。
    ///
    /// # Errors
    ///
    /// 没配置（`NotConfigured`）、无 compute/被关闭（`Unsupported`）、dispatch 校验失败。
    /// 池里某个系统 dispatch 失败时：其余系统照常步进，出错的那个当场作废（不带着可能
    /// 半更新的缓冲继续画），错误作为返回值上报。
    pub fn step_gpu_particles(
        &mut self,
        delta_seconds: f32,
    ) -> Result<GpuParticleStep, ComputeError> {
        if !self.compute_enabled() {
            return Err(self.compute_not_available());
        }
        if self.gpu_particles.is_empty() {
            return Err(ComputeError::NotConfigured);
        }
        if !delta_seconds.is_finite() || delta_seconds <= 0.0 {
            return Ok(self.gpu_particle_progress(0));
        }
        let mut failed = None;
        for index in 0..self.gpu_particles.len() {
            let system = &self.gpu_particles[index];
            let params = Self::gpu_particle_params(&system.emitter, delta_seconds);
            self.queue
                .write_buffer(&system.params_buffer, 0, bytemuck::bytes_of(&params));
            let workgroups = system.capacity.div_ceil(WORKGROUP_SIZE);
            // `dispatch_compute` 要 `&mut self`，而 `bindings` 借用 `system` 的字段。wgpu 的
            // 缓冲句柄是 Clone（内部引用计数），先 clone 成局部句柄即可结束对 `self` 的借用。
            let state_buffer = system.state_buffer.clone();
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
                    ComputeBinding::Uniform {
                        binding: 2,
                        buffer: &params_buffer,
                    },
                ],
                workgroups: [workgroups, 1, 1],
                bind_group_key: Some(bind_group_key),
            };
            match self.dispatch_compute(&request) {
                Ok(_) => {
                    let system = &mut self.gpu_particles[index];
                    system.steps += 1;
                    system.elapsed += delta_seconds;
                }
                Err(error) => {
                    // 标记成"已过寿命"，交给下面的回收通道；错误只上报第一个。
                    self.gpu_particles[index].elapsed = f32::INFINITY;
                    if failed.is_none() {
                        failed = Some(error);
                    }
                }
            }
        }
        let recycled = self.recycle_expired_gpu_particles();
        let step = self.gpu_particle_progress(recycled);
        match failed {
            Some(error) => {
                tracing::warn!(
                    target: "render",
                    error = %error,
                    recycled = step.recycled,
                    "[render] GPU 粒子 dispatch 失败：该系统已作废（其余系统照常推进）"
                );
                Err(error)
            }
            None => Ok(step),
        }
    }

    /// 本帧读数：`steps` 取池里最大值（最老的系统），`recycled` 是本次被回收的系统数。
    fn gpu_particle_progress(&self, recycled: usize) -> GpuParticleStep {
        GpuParticleStep {
            steps: self
                .gpu_particles
                .iter()
                .map(|system| system.steps)
                .max()
                .unwrap_or(0),
            systems: self.gpu_particles.len(),
            recycled,
        }
    }

    /// 回收寿命走完的系统，并精确剔除它们的绑定组缓存。
    ///
    /// 只剔绑定组、不动管线：管线 key 不含缓冲身份，回收一个消费者不该让别的消费者重编译。
    fn recycle_expired_gpu_particles(&mut self) -> usize {
        let before = self.gpu_particles.len();
        let mut retired_keys = Vec::new();
        self.gpu_particles.retain(|system| {
            if system.is_expired() {
                retired_keys.push(system.bind_group_key);
                false
            } else {
                true
            }
        });
        for key in retired_keys {
            self.compute_state.retire_bind_group(key);
        }
        before - self.gpu_particles.len()
    }

    /// 调试回读：把槽位状态拷回 CPU（仅 native；wasm 返回 `ReadbackUnsupported`）。
    ///
    /// 池里有多个系统时，按**入池顺序**把每个系统的存活槽位拼接起来——调用方要按
    /// [`GpuParticleStatus::spawned`] 的分布切片，别把两个 burst 的首尾当成同一批比较。
    ///
    /// # Errors
    ///
    /// 没配置、无 compute/被关闭、平台不支持、映射失败。
    pub fn read_gpu_particles(&self) -> Result<Vec<GpuParticleSnapshot>, ComputeError> {
        if !self.compute_enabled() {
            return Err(self.compute_not_available());
        }
        if self.gpu_particles.is_empty() {
            return Err(ComputeError::NotConfigured);
        }
        let mut snapshots = Vec::new();
        for system in &self.gpu_particles {
            let bytes = self.read_storage_buffer(&system.state_buffer, system.state_bytes)?;
            snapshots.extend(Self::gpu_particle_snapshots(&bytes, system.spawned));
        }
        Ok(snapshots)
    }

    /// 把回读的字节流解成快照，只取前 `spawned` 个槽位（容量里的空槽不是粒子）。
    fn gpu_particle_snapshots(bytes: &[u8], spawned: u32) -> Vec<GpuParticleSnapshot> {
        let mut snapshots = Vec::with_capacity(spawned as usize);
        for chunk in bytes.chunks_exact(std::mem::size_of::<GpuParticleState>()) {
            if snapshots.len() >= spawned as usize {
                break;
            }
            let state = bytemuck::pod_read_unaligned::<GpuParticleState>(chunk);
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
        snapshots
    }

    /// 池的聚合读数（诊断/验收用）：容量/粒子数/字节数是**求和**，`steps` 取池里最大值。
    /// 池为空时 `None`。
    #[must_use]
    pub fn gpu_particle_status(&self) -> Option<GpuParticleStatus> {
        if self.gpu_particles.is_empty() {
            return None;
        }
        let mut status = GpuParticleStatus {
            capacity: 0,
            spawned: 0,
            state_bytes: 0,
            backend: self.capabilities.backend_name(),
            steps: 0,
        };
        for system in &self.gpu_particles {
            status.capacity = status.capacity.saturating_add(system.capacity);
            status.spawned = status.spawned.saturating_add(system.spawned);
            status.state_bytes += system.state_bytes;
            status.steps = status.steps.max(system.steps);
        }
        Some(status)
    }

    /// 池里当前有多少个 GPU 粒子系统（派发通道据此判断"有没有东西要步进"）。
    #[must_use]
    pub fn gpu_particle_systems(&self) -> usize {
        self.gpu_particles.len()
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
    /// P3-R07-3 起走**自己的 instanced 管线**：4 顶点 triangle strip、无顶点缓冲、逐实例数据
    /// 是状态缓冲本身（group2）。几何（尺寸曲线、旋转、颜色插值）全在顶点着色器里算，compute
    /// 因此不再展开顶点——每粒子每帧的写带宽从 216 B 降到 0（只在活着时写 48 B 状态）。
    ///
    /// `group0`（相机）与 `group1`（白贴图）直接复用世界 Sprite 的两条绑定组，深度策略与
    /// alpha 混合也逐字段相同，所以半透明叠加的表现与 R07-3 之前一致。
    ///
    /// `hdr` 决定目标格式（`Rgba16Float` / 场景 LDR 格式），管线**按目标格式编译**并缓存一条：
    /// HDR 是一条真实变体，不是"用 LDR 管线画进 HDR 附件"——那条路径 wgpu 会在建 pass 时报
    /// 附件格式与管线不匹配，画面静默错位，所以这里按格式分叉而不是共用管线。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_gpu_particles(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        target_size: (u32, u32),
        world: WorldSpriteView,
        hdr: bool,
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
        if self.gpu_particles.is_empty() {
            return false;
        }
        let target_format = if hdr {
            wgpu::TextureFormat::Rgba16Float
        } else {
            self.config.format
        };
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
        // 管线要在 pass 之后取：`ensure_…` 借 `&mut self`，而 pass 的附件只借到 `begin_render_pass`
        // 返回为止，两者不冲突。取完之后 `pipeline` 立刻只用于 `set_pipeline`，随后的
        // `&self.world_sprite_camera_bind_group` 才能重新成立。
        let pipeline = self.ensure_gpu_particle_pipeline(target_format);
        pass.set_pipeline(pipeline);
        record_set_pass(metrics);
        pass.set_bind_group(0, &self.world_sprite_camera_bind_group, &[]);
        pass.set_bind_group(1, &white, &[]);
        apply_declared_viewport(&mut pass, world.viewport, target_size);
        // 一个 pass、每系统一次 draw：实例数 = 该系统的槽位数（死粒子由顶点着色器剔除，
        // 所以不需要每帧问 GPU"还活着几个"）。
        for system in &self.gpu_particles {
            if system.capacity == 0 {
                continue;
            }
            pass.set_bind_group(2, &system.draw_bind_group, &[]);
            pass.draw(0..QUAD_VERTEX_COUNT, 0..system.capacity);
            record_instanced_quad_draw(metrics, system.capacity);
        }
        true
    }

    /// 取（必要时编译）instanced 粒子管线。
    ///
    /// 缓存的是 `(格式, 管线)` 一条：HDR 帧与 LDR 帧交替出现时各编译一次，之后恒命中。
    /// 之所以延迟到第一次绘制才编译——启动预算（`compile_ms`）不该为"没配 GPU 粒子"的
    /// 进程付一条管线的钱，这与 P3-R02 的 PBR 延迟补编译是同一个理由。
    fn ensure_gpu_particle_pipeline(
        &mut self,
        format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        let stale = match &self.gpu_particle_pipeline {
            Some((cached, _)) => *cached != format,
            None => true,
        };
        if stale {
            let module = self
                .device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("rustlua-gpu-particles-draw-shader"),
                    source: wgpu::ShaderSource::Wgsl(GPU_PARTICLE_DRAW_SHADER.into()),
                });
            let layout = self
                .gpu_particle_pipeline_layout
                .as_ref()
                .expect("池里有系统 ⇒ 配置时已确认布局存在");
            let pipeline = build_gpu_particle_pipeline(
                &self.device,
                layout,
                &module,
                DepthPass::WorldSprite,
                format,
                "rustlua-gpu-particles-pipeline",
            );
            self.gpu_particle_pipeline = Some((format, pipeline));
        }
        let (_, pipeline) = self
            .gpu_particle_pipeline
            .as_ref()
            .expect("上面的分支刚刚写入过管线");
        pipeline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 状态/参数布局与 WGSL 对齐：这两条长度一旦漂移，画面上是"粒子错位/颜色乱"，
    /// 而 wgpu 的校验只会在管线创建时给一条难读的 stride 错误。
    #[test]
    fn gpu_layouts_match_wgsl_sizes() {
        assert_eq!(std::mem::size_of::<GpuParticleState>(), 48);
        assert_eq!(std::mem::size_of::<GpuParticleParams>(), 64);
    }

    /// compute 段与 draw 段必须**逐字节共享同一段声明**：它们绑的是同一块状态缓冲，
    /// `ParticleState` 一旦分叉，GPU 不会报错，画面只会变成"随机错乱的粒子"。
    ///
    /// 这条门钉的是 `particle_common!` 这个结构本身——把它换回两份常量文本，这里立刻红。
    #[test]
    fn compute_and_draw_share_one_state_declaration() {
        let common = particle_common!();
        assert!(
            GPU_PARTICLE_SHADER.starts_with(common),
            "compute 段的 ParticleState/Params 声明不再来自公共段"
        );
        assert!(
            GPU_PARTICLE_DRAW_SHADER.starts_with(common),
            "draw 段的 ParticleState/Params 声明不再来自公共段"
        );
        assert_eq!(
            common.matches("struct ").count(),
            2,
            "公共段应当只有 ParticleState 与 Params 两个结构"
        );
    }
}
