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
//! - **两条发射通道**：
//!   - `push_gpu_particle_burst[_3d]`：一次性 burst，CPU 生成初始状态后上传（Lua
//!     `particles.burst(..., {mode='gpu'})` 走这条）；
//!   - `push_gpu_particle_stream`：连续流，CPU 每帧按 `rate × dt` 抽样新粒子写进环形槽，
//!     GPU 只积分；场景声明的 `ParticleEmitter{rate>0,mode=gpu}` 走这条。连续流**只由宿主
//!     场景通道创建**，Lua burst API 没有 `rate` 选项；没有环形队列的后端仍由
//!     `ParticleEmitter::placement_with_streams` 回 CPU，不丢粒子。
//! - **不回读给 CPU 渲染**：compute 只写**状态**，几何在顶点着色器里逐实例展开，渲染侧零回读。
//!   `read_gpu_particles` 只给验收/调试用（native 同步回读）。
//! - **不改变默认画面**：没有调用 `configure_gpu_particles` 时，本模块一个 pass 都不产生；
//!   原有 CPU 粒子/精灵路径逐字节不变。
//!
//! ## 数据流
//!
//! ```text
//! burst：CPU 初始状态 → storage buffer（ParticleState × capacity，48 B/槽）
//! stream：CPU 每帧按 rate 抽样 → 环形槽（同一条 storage buffer 的空槽）
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
    position_z: f32,
    velocity_z: f32,
    life: f32,
    max_life: f32,
    size_start: f32,
    size_end: f32,
    rotation: f32,
    gravity_z: f32,
};

struct Params {
    dt: f32,
    drag: f32,
    rotation_speed: f32,
    wind_x: f32,
    gravity: vec2<f32>,
    wind_y: f32,
    wind_z: f32,
    wind_shear: f32,
    wind_gust: f32,
    wind_gust_hz: f32,
    time: f32,
    source_a: array<vec4<f32>, 4>,
    source_b: array<vec4<f32>, 4>,
    color_start: vec4<f32>,
    color_end: vec4<f32>,
    buoyancy_pad: vec4<f32>,
    curl_pad: vec4<f32>,
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
    // 与 CPU `ParticleSystem::wind_factor_at` 同一公式：高度剪切 + 空间正弦阵风。
    // `state.position` 是 (world x, world y=height)，`position_z` 是世界 z 深度。
    let gust_phase = state.position.y * params.wind_gust_hz + state.position.x * 0.11;
    let wind_factor = max(
        1.0 + params.wind_shear * state.position.y + params.wind_gust * sin(gust_phase),
        0.0,
    );
    var wind = vec2<f32>(params.wind_x, params.wind_y) * wind_factor;
    var wind_z = params.wind_z * wind_factor;
    // 与 CPU 同一全局三维湍流；strength 复用 gust，time 由宿主逐帧累加。
    let amp = params.wind_gust * 0.5;
    if (amp > 0.0) {
        let phase = state.position.y * params.wind_gust_hz + state.position.x * 0.11;
        wind.x = wind.x + amp * sin(phase * 1.7 + state.position_z * 0.13 + params.time * 0.7);
        wind_z = wind_z + amp * cos(phase * 1.3 - state.position_z * 0.09 + params.time * 0.5);
        wind.y = wind.y + amp * 0.5 * sin(phase * 2.1 + state.position.x * 0.2 + params.time * 0.9);
    }
    // 局部风源：与 CPU 同一套径向 + 上升公式（4 个固定槽，active 位在 source_b.z）。
    for (var index: u32 = 0u; index < 4u; index = index + 1u) {
        let source = params.source_a[index];
        let strength = params.source_b[index];
        if (strength.z < 0.5 || source.w <= 0.0) {
            continue;
        }
        let dx = state.position.x - source.x;
        let dy = state.position.y - source.y;
        let dz = state.position_z - source.z;
        let distance_sq = dx * dx + dy * dy + dz * dz;
        if (distance_sq >= source.w * source.w) {
            continue;
        }
        let distance = sqrt(distance_sq);
        let falloff = pow(1.0 - distance / source.w, 2.0);
        if (abs(strength.x) > 1e-9 && distance > 1e-4) {
            let radial = strength.x * falloff / distance;
            wind = wind + vec2<f32>(dx, dy) * radial;
            wind_z = wind_z + dz * radial;
        }
        if (abs(strength.w) > 1e-9 && distance > 1e-4) {
            let swirl = strength.w * falloff / distance;
            wind.x = wind.x - dz * swirl;
            wind_z = wind_z + dx * swirl;
        }
        wind.y = wind.y + strength.y * falloff;
    }
    // 程序化 curl 噪声：与 CPU `step_particle` 同一势函数旋度、同一相位来源。
    // 严格无散（∇·w = 0），所以只有旋转/剪切、没有源汇；除以 k 归一化后每分量 ≤ curl。
    let curl = params.curl_pad.x;
    if (curl > 0.0) {
        let k = max(params.curl_pad.y, 0.001);
        let w = params.time * 6.283185307179586 * params.curl_pad.z;
        let a = state.position.x * k + w;
        let b = state.position.y * k + w * 0.7;
        let c = state.position_z * k + w * 1.3;
        wind.x = wind.x + curl * sin(b) * sin(c);
        wind.y = wind.y + curl * cos(c) * sin(a);
        wind_z = wind_z + curl * sin(a) * sin(b);
    }
    // 逐粒子颤动：与 CPU `step_particle` 同一公式与同一相位来源（寿命/尺寸抖动的纯算术哈希）。
    // 必须在 state.velocity 积分之前修改 wind，否则 x/y 分量会被丢掉。
    if (params.buoyancy_pad.y > 0.0) {
        let phase = fract(state.max_life * 0.37 + state.size_start * 0.61);
        let wave = params.time * 6.283185307179586 * params.buoyancy_pad.z
            + phase * 6.283185307179586;
        wind.x = wind.x + params.buoyancy_pad.y * sin(wave);
        wind_z = wind_z + params.buoyancy_pad.y * cos(wave * 0.87 - phase * 6.283185307179586 * 1.7);
        wind.y = wind.y + params.buoyancy_pad.y * 0.45
            * sin(wave * 1.13 + phase * 6.283185307179586 * 0.6);
    }
    state.velocity = state.velocity + (params.gravity + wind) * dt;
    let buoyancy = params.buoyancy_pad.x * clamp(state.life / max(state.max_life, 1e-6), 0.0, 1.0);
    state.velocity_z = state.velocity_z + (state.gravity_z + wind_z + buoyancy) * dt;
    if (params.drag > 0.0) {
        let damp = max(1.0 - params.drag * dt, 0.0);
        state.velocity = state.velocity * damp;
        state.velocity_z = state.velocity_z * damp;
    }
    if (params.buoyancy_pad.w != 0.0) {
        // 沿寿命弯曲：与 CPU `step_particle` 同一 ramp（剩余寿命比例）与同一旋转公式，
        // 只转水平面 (x, y) 的方向、不改速率；z 是竖直轴，不参与。
        let ramp = clamp(state.life / max(state.max_life, 1e-6), 0.0, 1.0);
        let angle = params.buoyancy_pad.w * ramp * dt;
        let cs = cos(angle);
        let sn = sin(angle);
        let vx = state.velocity.x;
        let vy = state.velocity.y;
        state.velocity = vec2<f32>(vx * cs - vy * sn, vx * sn + vy * cs);
    }
    state.position = state.position + state.velocity * dt;
    state.position_z = state.position_z + state.velocity_z * dt;
    state.rotation = state.rotation + params.rotation_speed * dt;
    state.life = state.life - dt;
    states[index] = state;
}
"#
);

/// 网格体积风/温度浮力场的 GPU 声明段（P3-R234）：`GridMeta` uniform + 只读存储格阵 +
/// 三线性采样函数。`grid_data` 每格一个 `vec4`（xyz=速度，w=温度），线性顺序与 CPU
/// `WindGrid::index(x,y,z)=x+dims[0]*(y+dims[1]*z)` 逐格对应。采样逐式复刻 CPU
/// `particle_wind_grid::sample_accel`/`sample_coords`（盒外 clamp-to-edge、`+y` 浮力）。
const GPU_GRID_DECLS: &str = r#"

struct GridMeta {
    dims: vec4<u32>,
    origin_cell: vec4<f32>,
    buoyancy_pad: vec4<f32>,
};

@group(0) @binding(1) var<uniform> grid_meta: GridMeta;
@group(0) @binding(3) var<storage, read> grid_data: array<vec4<f32>>;

fn grid_index(cx: u32, cy: u32, cz: u32) -> u32 {
    return cx + grid_meta.dims.x * (cy + grid_meta.dims.y * cz);
}

fn sample_grid_accel(point: vec3<f32>) -> vec3<f32> {
    let dims = grid_meta.dims.xyz;
    let cell = grid_meta.origin_cell.w;
    let origin = grid_meta.origin_cell.xyz;
    let local = (point - origin) / cell;
    let max_x = dims.x - 1u;
    let gx = clamp(local.x, 0.0, f32(max_x));
    let bx = u32(floor(gx));
    let c0x = min(bx, max_x);
    let hix = min(bx + 1u, max_x);
    let fx = gx - f32(bx);
    let max_y = dims.y - 1u;
    let gy = clamp(local.y, 0.0, f32(max_y));
    let by = u32(floor(gy));
    let c0y = min(by, max_y);
    let hiy = min(by + 1u, max_y);
    let fy = gy - f32(by);
    let max_z = dims.z - 1u;
    let gz = clamp(local.z, 0.0, f32(max_z));
    let bz = u32(floor(gz));
    let c0z = min(bz, max_z);
    let hiz = min(bz + 1u, max_z);
    let fz = gz - f32(bz);
    var vel = vec3<f32>(0.0);
    var temp = 0.0;
    for (var iz: u32 = 0u; iz < 2u; iz = iz + 1u) {
        let cz = select(c0z, hiz, iz == 1u);
        let wz = select(1.0 - fz, fz, iz == 1u);
        for (var iy: u32 = 0u; iy < 2u; iy = iy + 1u) {
            let cy = select(c0y, hiy, iy == 1u);
            let wy = select(1.0 - fy, fy, iy == 1u);
            for (var ix: u32 = 0u; ix < 2u; ix = ix + 1u) {
                let cx = select(c0x, hix, ix == 1u);
                let wx = select(1.0 - fx, fx, ix == 1u);
                let w = wx * wy * wz;
                let cellv = grid_data[grid_index(cx, cy, cz)];
                vel = vel + cellv.xyz * w;
                temp = temp + cellv.w * w;
            }
        }
    }
    return vel + vec3<f32>(0.0, grid_meta.buoyancy_pad.x * temp, 0.0);
}
"#;

/// 注入到积分前的采样块：与 CPU `step_particle` 同一落点（curl/flutter/风源之后、
/// `state.velocity` 积分之前），把网格加速度叠进 `wind`/`wind_z`。
const GPU_GRID_SAMPLE: &str = r#"    let grid_accel = sample_grid_accel(vec3<f32>(state.position.x, state.position.y, state.position_z));
    wind.x = wind.x + grid_accel.x;
    wind.y = wind.y + grid_accel.y;
    wind_z = wind_z + grid_accel.z;
"#;

/// 有网格时的 compute 变体：**从 [`GPU_PARTICLE_SHADER`] 派生**而非另写一份——共享段
/// （状态声明 + 全部风/湍流/积分逻辑）逐字节同一份，不可能分叉。只在两处锚点插入网格
/// 声明与采样块。无网格时宿主仍用原 [`GPU_PARTICLE_SHADER`]，默认路径逐位不变。
///
/// 锚点失配会让 `replace` 静默 no-op，[`tests::gpu_grid_shader_derives_from_base`] 钉住
/// "派生真的发生了"（变体含采样函数且比基段长）。
fn gpu_particle_grid_shader() -> &'static str {
    static GRID_SHADER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    GRID_SHADER.get_or_init(|| {
        const PARAMS_BINDING: &str = "@group(0) @binding(2) var<uniform> params: Params;";
        const INTEGRATION: &str =
            "    state.velocity = state.velocity + (params.gravity + wind) * dt;";
        let with_decls = GPU_PARTICLE_SHADER
            .replace(PARAMS_BINDING, &format!("{PARAMS_BINDING}{GPU_GRID_DECLS}"));
        with_decls.replace(INTEGRATION, &format!("{GPU_GRID_SAMPLE}{INTEGRATION}"))
    })
}

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
// P3-R107：场景深度（3D pass 已结束，深度纹理只作为纹理采样，不再挂到本 pass 上）。
@group(3) @binding(0) var scene_depth: texture_depth_2d;

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
    var world = vec3<f32>(state.position, state.position_z);
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
    let dims = vec2<i32>(textureDimensions(scene_depth));
    let frag = in.position;
    let coord = clamp(vec2<i32>(frag.xy), vec2<i32>(0), dims - vec2<i32>(1));
    let scene = textureLoad(scene_depth, coord, 0);
    let delta = scene - frag.z;
    // 粒子在场景几何后面：手动深度剔除（本 pass 不再挂深度附件）。
    if delta < -0.0001 {
        return vec4<f32>(0.0);
    }
    // 靠近遮挡面时软边淡出，烟/火/雨不再出现硬切边。
    let soft = smoothstep(0.0, 0.02, delta);
    let texel = textureSample(tex, samp, in.uv) * in.color;
    return vec4<f32>(texel.rgb, texel.a * soft);
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
    pub position_z: f32,
    pub velocity_z: f32,
    pub life: f32,
    pub max_life: f32,
    pub size_start: f32,
    pub size_end: f32,
    pub rotation: f32,
    pub gravity_z: f32,
}

impl GpuParticleState {
    fn from_cpu(particle: &rustlua_ecs::particle::Particle) -> Self {
        Self {
            position: [particle.position.x, particle.position.y],
            velocity: [particle.velocity.x, particle.velocity.y],
            position_z: particle.position_z,
            velocity_z: particle.velocity_z,
            life: particle.life,
            max_life: particle.max_life,
            size_start: particle.size_start,
            size_end: particle.size_end,
            rotation: particle.rotation,
            gravity_z: particle.gravity_z,
        }
    }
}

/// 一次 GPU 粒子的配置。
///
/// 语义与 [`ParticleEmitter`] 一致；`capacity` 是显存里预留的槽位（≥ `emitter.count`）。
/// `rate > 0` 会被 burst 入口**拒绝**；连续发射用 [`GpuParticleStreamConfig`]。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuParticleConfig {
    /// burst 的世界坐标（2D）。
    pub origin: [f32; 2],
    /// 与 CPU 粒子系统共用同一份发射参数（单一口径）。
    pub emitter: ParticleEmitter,
    /// GPU 槽位上限；`emitter.count` 必须 ≤ 它。
    pub capacity: u32,
}

/// 三维 burst 的配置。语义与 [`GpuParticleConfig`] 相同，只把原点换成 `[x, y, z]`。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuParticleConfig3D {
    pub origin: [f32; 3],
    pub emitter: ParticleEmitter,
    pub capacity: u32,
}

/// 配置成功后的读数。
///
/// P3-R07-3 起没有 `vertex_bytes` 这一项了：instanced 路径的**实例流就是状态缓冲**，
/// 两者是同一块显存，再报一个"顶点字节"只会读成两份并不存在的开销。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuParticleStatus {
    /// 预留槽位（burst 与连续流求和）。
    pub capacity: u32,
    /// burst 的已写槽数；连续流按整个环形容量报（槽位里可能有已死粒子）。
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
    pub position_z: f32,
    pub velocity: [f32; 2],
    pub velocity_z: f32,
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
    wind_x: f32,
    gravity: [f32; 2],
    wind_y: f32,
    wind_z: f32,
    wind_shear: f32,
    wind_gust: f32,
    wind_gust_hz: f32,
    time: f32,
    source_a: [[f32; 4]; 4],
    source_b: [[f32; 4]; 4],
    color_start: [f32; 4],
    color_end: [f32; 4],
    buoyancy_pad: [f32; 4],
    curl_pad: [f32; 4],
}

/// 网格风场的 GPU 元数据（与 WGSL `GridMeta` 逐字节对应，48 字节）。全 `vec4` 避开
/// uniform 里 `vec3` 的 16 字节对齐陷阱：`dims.xyz`/`origin_cell.xyz` 是网格维度与世界
/// 原点，`origin_cell.w` 是节点间距，`buoyancy_pad.x` 是温度浮力增益。
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GpuGridMeta {
    dims: [u32; 4],
    origin_cell: [f32; 4],
    buoyancy_pad: [f32; 4],
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
    /// 连续流状态；`None` = 普通 burst（寿命到点回收）。
    stream: Option<GpuParticleStream>,
}

/// 一个连续发射器的环形槽状态。
///
/// 新粒子由 CPU 按 `rate × dt` 抽样后写进空槽，GPU 只负责后续积分；槽位写满后从
/// 头覆盖。`sampler` 只用来抽样，不保存模拟结果，因此 `clear()` 后不会随着运行时间
/// 越长内存越大。
pub(super) struct GpuParticleStream {
    key: u64,
    cursor: u32,
    accumulator: f32,
    origin: [f32; 3],
    sampler: ParticleSystem,
}

/// 渲染器持有的网格风场显存副本（P3-R234）。场本身在 CPU 侧 `WindGrid` 推进（宿主每帧
/// `particles.step` 里），这里只存上传后的格阵 + 元数据，供 compute 三线性采样。
///
/// `data_buffer`/`meta_buffer` 的句柄在格数不变时**保持稳定**（每帧只 `write_buffer` 内容），
/// 这样按 `(pipeline_key, system_key)` 缓存的绑定组不会引用到旧缓冲。格数变化（重建网格）
/// 时重分配并剔除所有粒子绑定组缓存。`staging` 复用同一块打包内存，避免逐帧分配。
pub(super) struct GpuParticleGrid {
    data_buffer: wgpu::Buffer,
    meta_buffer: wgpu::Buffer,
    cell_count: u32,
    staging: Vec<f32>,
}

/// 连续粒子流的创建参数。
#[derive(Clone, Copy, Debug)]
pub struct GpuParticleStreamConfig {
    /// 发射原点（世界坐标，z 可非零）。
    pub origin: [f32; 3],
    /// 发射参数；`rate` 必须 `> 0`，`count` 不参与容量计算。
    pub emitter: ParticleEmitter,
    /// 环形容量；`0` = 按 `rate × life × 1.25` 自动取（至少 8 槽）。
    pub capacity: u32,
}

impl GpuParticleSystem {
    pub(super) const fn is_active(&self) -> bool {
        self.capacity > 0
    }

    /// 寿命是否已走完（池回收判据）。
    ///
    /// 连续流没有"整批寿命"：只要宿主还声明它就一直活着；只有 dispatch 失败
    /// （`elapsed = INFINITY`）才回收。
    pub(super) fn is_expired(&self) -> bool {
        if self.stream.is_some() {
            return self.elapsed == f32::INFINITY;
        }
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
    /// 能力缺失、`emitter.rate > 0`（连续发射请用 [`Self::push_gpu_particle_stream`]）、
    /// 容量/绑定超限、参数非有限。
    pub fn configure_gpu_particles(
        &mut self,
        config: GpuParticleConfig,
    ) -> Result<GpuParticleStatus, ComputeError> {
        if !self.compute_enabled() {
            return Err(self.compute_not_available());
        }
        let system = self.build_gpu_particle_system(config, 0.0)?;
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
        self.push_gpu_particle_burst_at(config, 0.0)
    }

    /// 三维 burst：`origin[2]` 原样进入初始状态；其余与
    /// [`Self::push_gpu_particle_burst`] 同一套池预算/校验。
    pub fn push_gpu_particle_burst_3d(
        &mut self,
        config: GpuParticleConfig3D,
    ) -> Result<GpuParticleStatus, ComputeError> {
        self.push_gpu_particle_burst_at(
            GpuParticleConfig {
                origin: [config.origin[0], config.origin[1]],
                emitter: config.emitter,
                capacity: config.capacity,
            },
            config.origin[2],
        )
    }

    fn push_gpu_particle_burst_at(
        &mut self,
        config: GpuParticleConfig,
        origin_z: f32,
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
        let system = self.build_gpu_particle_system(config, origin_z)?;
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
        origin_z: f32,
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
        cpu.burst_with_3d(
            [config.origin[0], config.origin[1], origin_z],
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
        let params = Self::gpu_particle_params(
            &config.emitter,
            0.0,
            self.gpu_particle_wind,
            self.gpu_particle_wind_z,
            self.gpu_particle_wind_profile(),
            self.gpu_particle_wind_sources,
            self.gpu_particle_time,
        );
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
            stream: None,
        })
    }

    /// 创建一个**连续** GPU 粒子流：槽位由 CPU 按速率写入、GPU 负责积分，直到宿主动
    /// [`Self::remove_gpu_particle_stream`]。
    ///
    /// `key` 由宿主分配（场景实体 id 即可），同一个 key 重复创建会被拒绝而不是悄悄替换；
    /// 宿主换发射器参数时应先删再建。
    ///
    /// # Errors
    ///
    /// 无 compute、key 重复、`rate <= 0`、参数非有限、容量/槽位/绑定超限。
    pub fn push_gpu_particle_stream(
        &mut self,
        key: u64,
        config: GpuParticleStreamConfig,
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
        if self.gpu_particles.iter().any(|system| {
            system
                .stream
                .as_ref()
                .is_some_and(|stream| stream.key == key)
        }) {
            return Err(ComputeError::InvalidConfig(format!(
                "GPU 粒子流 key={key} 已存在（先删再建）"
            )));
        }
        let system = self.build_gpu_particle_stream(key, config)?;
        let status = system.status(self.capabilities.backend_name());
        self.gpu_particles.push(system);
        Ok(status)
    }

    /// 移动一个已存在的连续流；返回 false 表示 key 不在池里（宿主应重建）。
    pub fn set_gpu_particle_stream_origin(&mut self, key: u64, origin: [f32; 3]) -> bool {
        if !origin.iter().all(|value| value.is_finite()) {
            return false;
        }
        let Some(system) = self.gpu_particles.iter_mut().find(|system| {
            system
                .stream
                .as_ref()
                .is_some_and(|stream| stream.key == key)
        }) else {
            return false;
        };
        let Some(stream) = system.stream.as_mut() else {
            return false;
        };
        stream.origin = origin;
        true
    }

    /// 删除一个连续流；返回 false 表示 key 本来就不在池里。
    pub fn remove_gpu_particle_stream(&mut self, key: u64) -> bool {
        let Some(index) = self.gpu_particles.iter().position(|system| {
            system
                .stream
                .as_ref()
                .is_some_and(|stream| stream.key == key)
        }) else {
            return false;
        };
        let system = self.gpu_particles.remove(index);
        self.compute_state.retire_bind_group(system.bind_group_key);
        true
    }

    /// 当前池里的连续流 key（按入池顺序；测试/诊断用）。
    #[must_use]
    pub fn gpu_particle_stream_keys(&self) -> Vec<u64> {
        self.gpu_particles
            .iter()
            .filter_map(|system| system.stream.as_ref().map(|stream| stream.key))
            .collect()
    }

    /// 连续流个数。
    #[must_use]
    pub fn gpu_particle_stream_count(&self) -> usize {
        self.gpu_particles
            .iter()
            .filter(|system| system.stream.is_some())
            .count()
    }

    fn build_gpu_particle_stream(
        &mut self,
        key: u64,
        config: GpuParticleStreamConfig,
    ) -> Result<GpuParticleSystem, ComputeError> {
        config
            .emitter
            .validate()
            .map_err(|error| ComputeError::InvalidConfig(error.to_owned()))?;
        if config.emitter.rate <= 0.0 || !config.emitter.rate.is_finite() {
            return Err(ComputeError::InvalidConfig(
                "连续流的 emitter.rate 必须是有限正数".to_owned(),
            ));
        }
        if !config.origin.iter().all(|value| value.is_finite()) {
            return Err(ComputeError::InvalidConfig(
                "连续流的 origin 必须是有限数".to_owned(),
            ));
        }
        if self.gpu_particle_draw_layout.is_none() {
            return Err(ComputeError::Unsupported {
                reason: "设备没有存储缓冲（max_storage_buffers_per_shader_stage=0）：\
                         GPU 粒子 draw 布局未创建，走 CPU 粒子"
                    .to_owned(),
            });
        }
        let capacity = if config.capacity == 0 {
            let estimate = (config.emitter.rate * config.emitter.life * 1.25).ceil();
            if !estimate.is_finite() || estimate > MAX_GPU_PARTICLE_SLOTS as f32 {
                return Err(ComputeError::InvalidConfig(format!(
                    "rate×life={} 超过 GPU 粒子槽位上限 {MAX_GPU_PARTICLE_SLOTS}",
                    estimate
                )));
            }
            (estimate as u32).max(8)
        } else {
            config.capacity
        };
        let used = self.gpu_particle_slots_used();
        let total = used.saturating_add(capacity);
        if total > MAX_GPU_PARTICLE_SLOTS {
            return Err(ComputeError::InvalidConfig(format!(
                "GPU 粒子槽位预算不足：已用 {used} + 连续流 {capacity} = {total}，\
                 超过上限 {MAX_GPU_PARTICLE_SLOTS}"
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
        let workgroups = capacity.div_ceil(WORKGROUP_SIZE);
        if workgroups > self.capabilities.max_compute_workgroups_per_dimension {
            return Err(ComputeError::InvalidConfig(format!(
                "capacity={capacity} 需要 {workgroups} 个 workgroup，超过设备上限 {}",
                self.capabilities.max_compute_workgroups_per_dimension
            )));
        }
        let state_bytes = u64::from(capacity) * std::mem::size_of::<GpuParticleState>() as u64;
        if state_bytes > self.capabilities.max_storage_buffer_binding_size {
            return Err(ComputeError::BindingTooLarge {
                binding: 0,
                requested: state_bytes,
                limit: self.capabilities.max_storage_buffer_binding_size,
            });
        }
        let states = vec![GpuParticleState::zeroed(); capacity as usize];
        let state_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-gpu-particles-stream-state"),
                contents: bytemuck::cast_slice(&states),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            });
        let params = Self::gpu_particle_params(
            &config.emitter,
            0.0,
            self.gpu_particle_wind,
            self.gpu_particle_wind_z,
            self.gpu_particle_wind_profile(),
            self.gpu_particle_wind_sources,
            self.gpu_particle_time,
        );
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-gpu-particles-stream-params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let draw_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-gpu-particles-stream-draw"),
            layout: self
                .gpu_particle_draw_layout
                .as_ref()
                .expect("build_gpu_particle_stream 入口已拒绝无布局的设备"),
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
            capacity,
            // 连续流的槽位是环形池；回读/诊断按整个容量看，不按"累计发射过多少"。
            spawned: capacity,
            steps: 0,
            elapsed: 0.0,
            state_buffer,
            draw_bind_group,
            params_buffer,
            bind_group_key: self.next_gpu_particle_key(),
            state_bytes,
            stream: Some(GpuParticleStream {
                key,
                cursor: 0,
                accumulator: 0.0,
                origin: config.origin,
                sampler: ParticleSystem::new(),
            }),
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
    /// 每帧末尾回收寿命走完的 burst 系统（缓冲释放 + 绑定组缓存条目按 id 精确剔除）。回收判据是
    /// `elapsed > emitter.life`，而 CPU 侧任何粒子的 `max_life` 都 ≤ `emitter.life`，因此
    /// 这一步**不需要读回**——GPU 粒子的"什么时候全死了"由发射参数就能推出上界。连续流没有
    /// 整批寿命，只在 dispatch 失败（`elapsed = INFINITY`）时回收，实体消失由宿主删流。
    ///
    /// # Errors
    ///
    /// 没配置（`NotConfigured`）、无 compute/被关闭（`Unsupported`）、dispatch 校验失败。
    /// 池里某个系统 dispatch 失败时：其余系统照常步进，出错的那个当场作废（不带着可能
    /// 半更新的缓冲继续画），错误作为返回值上报。
    /// 连续流按 `rate × dt` 抽样新粒子并写进环形槽；dead 槽位不碰。
    ///
    /// 写缓冲只写新粒子那几十/几百字节，不做整池上传。抽样器在 `clear()` 后继续推进
    /// LCG，所以连续发射不会每帧拿到同一组随机数。
    fn spawn_stream_particles(&mut self, index: usize, delta_seconds: f32) {
        let queue = self.queue.clone();
        let Some(system) = self.gpu_particles.get_mut(index) else {
            return;
        };
        let Some(stream) = system.stream.as_mut() else {
            return;
        };
        stream.accumulator += system.emitter.rate * delta_seconds;
        let wanted = stream.accumulator.floor();
        if !wanted.is_finite() || wanted <= 0.0 {
            return;
        }
        stream.accumulator -= wanted;
        let capacity = system.capacity as usize;
        let count = (wanted as usize).min(capacity);
        if count == 0 {
            return;
        }
        let mut emitter = system.emitter;
        emitter.count = count;
        // 连续发射要不断前进；固定 seed 只对单次 burst 有意义。
        emitter.seed = None;
        stream.sampler.clear();
        stream.sampler.burst_with_3d(stream.origin, &emitter);
        if stream.sampler.len() != count {
            return;
        }
        let states: Vec<GpuParticleState> = stream
            .sampler
            .particles()
            .iter()
            .map(GpuParticleState::from_cpu)
            .collect();
        let state_size = std::mem::size_of::<GpuParticleState>() as u64;
        for (offset, state) in states.iter().enumerate() {
            let slot = (stream.cursor as usize + offset) % capacity;
            let byte_offset = slot as u64 * state_size;
            queue.write_buffer(&system.state_buffer, byte_offset, bytemuck::bytes_of(state));
        }
        stream.cursor = ((stream.cursor as usize + states.len()) % capacity) as u32;
    }

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
        self.gpu_particle_time += delta_seconds;
        let mut failed = None;
        // 网格风场是全局一份（宿主每帧从 CPU `WindGrid` 上传）：句柄克隆成局部 owned，
        // 结束对 `self` 的借用，循环里再按系统拼绑定。无网格 ⇒ 用原 shader、原两条绑定，
        // 默认路径逐位不变；有网格 ⇒ 派生变体 + binding1/3 两条额外绑定。
        let grid_buffers = self
            .gpu_particle_grid
            .as_ref()
            .map(|grid| (grid.meta_buffer.clone(), grid.data_buffer.clone()));
        let grid_binding_refs = grid_buffers.as_ref();
        let shader = if grid_binding_refs.is_some() {
            gpu_particle_grid_shader()
        } else {
            GPU_PARTICLE_SHADER
        };
        for index in 0..self.gpu_particles.len() {
            if self.gpu_particles[index].stream.is_some() {
                self.spawn_stream_particles(index, delta_seconds);
            }
            let system = &self.gpu_particles[index];
            let params = Self::gpu_particle_params(
                &system.emitter,
                delta_seconds,
                self.gpu_particle_wind,
                self.gpu_particle_wind_z,
                self.gpu_particle_wind_profile(),
                self.gpu_particle_wind_sources,
                self.gpu_particle_time,
            );
            self.queue
                .write_buffer(&system.params_buffer, 0, bytemuck::bytes_of(&params));
            let workgroups = system.capacity.div_ceil(WORKGROUP_SIZE);
            // `dispatch_compute` 要 `&mut self`，而 `bindings` 借用 `system` 的字段。wgpu 的
            // 缓冲句柄是 Clone（内部引用计数），先 clone 成局部句柄即可结束对 `self` 的借用。
            let state_buffer = system.state_buffer.clone();
            let params_buffer = system.params_buffer.clone();
            let bind_group_key = system.bind_group_key;
            // 无网格时 binding1/3 用现成句柄占位，但被切片排除：不进 layout、不被校验。
            let (grid_meta_buffer, grid_data_buffer) = match grid_binding_refs {
                Some((meta, data)) => (meta, data),
                None => (&params_buffer, &state_buffer),
            };
            let bindings = [
                ComputeBinding::Storage {
                    binding: 0,
                    buffer: &state_buffer,
                    read_only: false,
                },
                ComputeBinding::Uniform {
                    binding: 2,
                    buffer: &params_buffer,
                },
                ComputeBinding::Uniform {
                    binding: 1,
                    buffer: grid_meta_buffer,
                },
                ComputeBinding::Storage {
                    binding: 3,
                    buffer: grid_data_buffer,
                    read_only: true,
                },
            ];
            let binding_count = if grid_binding_refs.is_some() { 4 } else { 2 };
            let request = ComputeRequest {
                label: "rustlua-gpu-particles-step",
                shader,
                entry_point: "step",
                bindings: &bindings[..binding_count],
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
                position_z: state.position_z,
                velocity: state.velocity,
                velocity_z: state.velocity_z,
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

    /// 设置 GPU burst 共用的全局风加速度；非有限值忽略（保持上一份合法风）。
    pub fn set_gpu_particle_wind(&mut self, wind: [f32; 2]) {
        if wind[0].is_finite() && wind[1].is_finite() {
            self.gpu_particle_wind = wind;
        }
    }

    /// 设置三维全局风；Z 分量只影响 `position_z` 的积分，与 CPU 池同口径。
    pub fn set_gpu_particle_wind_3d(&mut self, wind: [f32; 3]) {
        if wind[0].is_finite() && wind[1].is_finite() && wind[2].is_finite() {
            self.gpu_particle_wind = [wind[0], wind[1]];
            self.gpu_particle_wind_z = wind[2];
        }
    }

    #[must_use]
    pub const fn gpu_particle_wind(&self) -> [f32; 2] {
        self.gpu_particle_wind
    }

    /// 设置风场 `(shear, gust, gust_hz)`；非有限/越界拒绝并保持旧值。
    ///
    /// 范围与 CPU `ParticleSystem::set_wind_profile` 一致：shear `-1..=1`、
    /// gust `0..=4`、gust_hz `0..=20`。返回是否写入。
    pub fn set_gpu_particle_wind_profile(&mut self, profile: [f32; 3]) -> bool {
        let [shear, gust, gust_hz] = profile;
        if !shear.is_finite()
            || !gust.is_finite()
            || !gust_hz.is_finite()
            || !(-1.0..=1.0).contains(&shear)
            || !(0.0..=4.0).contains(&gust)
            || !(0.0..=20.0).contains(&gust_hz)
        {
            return false;
        }
        self.gpu_particle_wind_shear = shear;
        self.gpu_particle_wind_gust = gust;
        self.gpu_particle_wind_gust_hz = gust_hz;
        true
    }

    /// 当前风场 `(shear, gust, gust_hz)`。
    #[must_use]
    pub const fn gpu_particle_wind_profile(&self) -> [f32; 3] {
        [
            self.gpu_particle_wind_shear,
            self.gpu_particle_wind_gust,
            self.gpu_particle_wind_gust_hz,
        ]
    }

    /// 设置 4 个局部风源 `[x, y(height), z, radius, radial, lift, swirl]`；任一非法则整批拒绝。
    pub fn set_gpu_particle_wind_sources(&mut self, sources: [[f32; 7]; 4]) -> bool {
        let valid = sources.iter().all(|source| {
            source.iter().all(|value| value.is_finite())
                && (0.0..=512.0).contains(&source[3])
                && (-64.0..=64.0).contains(&source[4])
                && (-64.0..=64.0).contains(&source[5])
                && (-64.0..=64.0).contains(&source[6])
        });
        if !valid {
            return false;
        }
        self.gpu_particle_wind_sources = sources;
        true
    }

    /// 当前局部风源。
    #[must_use]
    pub const fn gpu_particle_wind_sources(&self) -> [[f32; 7]; 4] {
        self.gpu_particle_wind_sources
    }

    #[must_use]
    pub const fn gpu_particle_wind_3d(&self) -> [f32; 3] {
        [
            self.gpu_particle_wind[0],
            self.gpu_particle_wind[1],
            self.gpu_particle_wind_z,
        ]
    }

    /// 把 CPU 侧网格风场同步给 GPU 粒子后端（P3-R234）。
    ///
    /// 场本身在 CPU 侧推进（宿主每帧 `particles.step` 里 `WindGrid::step`），这里只上传
    /// 一份显存副本供 compute 三线性采样——**单一模拟源**，CPU/GPU 采样的是同一份场。
    /// `None` = 无场 ⇒ GPU 走原 shader（无网格绑定），逐位不变。
    ///
    /// 格数不变时缓冲句柄保持稳定（每帧只写内容），绑定组缓存不失效；格数变了才重分配，
    /// 并剔除所有粒子绑定组缓存——否则缓存里的绑定组还指着旧缓冲，场会"读到自己上一世"。
    pub fn set_gpu_particle_wind_grid(
        &mut self,
        grid: Option<&rustlua_ecs::particle_wind_grid::WindGrid>,
    ) {
        let Some(grid) = grid else {
            if self.gpu_particle_grid.take().is_some() {
                self.retire_gpu_particle_bind_groups();
            }
            return;
        };
        // 没有 compute 的设备（WebGL2）根本走不到 GPU 粒子，别为它建存储缓冲。
        if !self.compute_enabled() {
            return;
        }
        let cell_count = grid.cell_count();
        let dims = grid.dims();
        let needs_new_buffer = self
            .gpu_particle_grid
            .as_ref()
            .is_none_or(|current| current.cell_count as usize != cell_count);
        if needs_new_buffer {
            let data_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-gpu-particle-grid"),
                size: (cell_count * std::mem::size_of::<[f32; 4]>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let meta_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-gpu-particle-grid-meta"),
                size: std::mem::size_of::<GpuGridMeta>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.gpu_particle_grid = Some(GpuParticleGrid {
                data_buffer,
                meta_buffer,
                cell_count: cell_count as u32,
                staging: vec![0.0; cell_count * 4],
            });
            self.retire_gpu_particle_bind_groups();
        }
        let Some(current) = self.gpu_particle_grid.as_mut() else {
            return;
        };
        // 打包：每格 `vec4` = (vx, vy, vz, 温度)；线性顺序与 CPU `WindGrid::index` 逐格对应。
        let velocity = grid.velocity_data();
        let temperature = grid.temperature_data();
        for (index, (v, &t)) in velocity.iter().zip(temperature.iter()).enumerate() {
            let base = index * 4;
            current.staging[base] = v.x;
            current.staging[base + 1] = v.y;
            current.staging[base + 2] = v.z;
            current.staging[base + 3] = t;
        }
        let meta = GpuGridMeta {
            dims: [dims[0] as u32, dims[1] as u32, dims[2] as u32, 0],
            origin_cell: [
                grid.origin().x,
                grid.origin().y,
                grid.origin().z,
                grid.cell(),
            ],
            buoyancy_pad: [grid.buoyancy(), 0.0, 0.0, 0.0],
        };
        self.queue.write_buffer(
            &current.data_buffer,
            0,
            bytemuck::cast_slice(&current.staging),
        );
        self.queue
            .write_buffer(&current.meta_buffer, 0, bytemuck::bytes_of(&meta));
    }

    /// 剔除池里所有粒子系统的绑定组缓存。
    ///
    /// 绑定组缓存键是 `(管线 key, 系统 id)`，**不含缓冲身份**——网格缓冲换了句柄后，
    /// 缓存里的绑定组还指着旧缓冲。网格重建（格数变化）时必须整体失效一次。
    fn retire_gpu_particle_bind_groups(&mut self) {
        let keys: Vec<u64> = self
            .gpu_particles
            .iter()
            .map(|system| system.bind_group_key)
            .collect();
        for key in keys {
            self.compute_state.retire_bind_group(key);
        }
    }

    fn gpu_particle_params(
        emitter: &ParticleEmitter,
        dt: f32,
        wind: [f32; 2],
        wind_z: f32,
        profile: [f32; 3],
        sources: [[f32; 7]; 4],
        time: f32,
    ) -> GpuParticleParams {
        let mut source_a = [[0.0f32; 4]; 4];
        let mut source_b = [[0.0f32; 4]; 4];
        for (index, source) in sources.iter().enumerate() {
            source_a[index] = [source[0], source[1], source[2], source[3]];
            source_b[index] = [
                source[4],
                source[5],
                if source[3] > 0.0 { 1.0 } else { 0.0 },
                source[6],
            ];
        }
        GpuParticleParams {
            dt,
            drag: emitter.drag,
            rotation_speed: emitter.rotation_speed,
            wind_x: wind[0],
            gravity: [emitter.gravity.x, emitter.gravity.y],
            wind_y: wind[1],
            wind_z,
            wind_shear: profile[0],
            wind_gust: profile[1],
            wind_gust_hz: profile[2],
            time,
            source_a,
            source_b,
            color_start: emitter.color_start,
            color_end: emitter.color_end,
            buoyancy_pad: [
                emitter.buoyancy,
                emitter.flutter,
                emitter.flutter_hz,
                emitter.curve,
            ],
            curl_pad: [emitter.curl, emitter.curl_scale, emitter.curl_hz, 0.0],
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
        // 深度不再挂到粒子 pass 上，需要清时单独开一个无颜色附件的清深度 pass。
        if clear_depth {
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-gpu-particles-depth-clear"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        // P3-R107：深度只作为 group3 纹理；本 pass 不再挂深度附件，片元里手动比较/软边。
        let depth_bind_group = self.gpu_particle_depth_layout.as_ref().map(|layout| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rustlua-gpu-particles-depth-bind-group"),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(depth_view),
                }],
            })
        });
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
            depth_stencil_attachment: None,
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
            if let Some(group) = &depth_bind_group {
                pass.set_bind_group(3, group, &[]);
            }
            pass.draw(0..QUAD_VERTEX_COUNT, 0..system.capacity);
            record_instanced_quad_draw(metrics, system.capacity);
        }
        true
    }

    /// 取（必要时编译）instanced 粒子管线。
    ///
    /// 缓存按**目标格式**分条：HDR 与 LDR 是两条真实变体，切换时各编译一次、之后恒命中。
    /// 默认仍是延迟到第一次绘制才编译——启动预算（`compile_ms`）不该为"没配 GPU 粒子"的
    /// 进程付一条管线的钱，这与 P3-R02 的 PBR 延迟补编译是同一个理由。
    /// P3-R243 另给了 [`Self::prewarm_gpu_particles`]：显式要求的宿主把这份钱挪到启动期。
    fn ensure_gpu_particle_pipeline(
        &mut self,
        format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        if !self
            .gpu_particle_pipelines
            .iter()
            .any(|(cached, _)| *cached == format)
        {
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
            self.gpu_particle_pipelines.push((format, pipeline));
        }
        let (_, pipeline) = self
            .gpu_particle_pipelines
            .iter()
            .find(|(cached, _)| *cached == format)
            .expect("上面的分支刚刚写入过管线");
        pipeline
    }

    /// P3-R243：把 instanced 粒子管线在两种目标格式上先编好，返回本次新编的条数。
    ///
    /// 动机：管线默认延迟到第一次画 GPU 粒子才编译（`compile_ms` 的启动预算），于是第一股
    /// 连续流出现的那一帧要付一次管线编译——雨/雪/酸雾一开场就开流，尖峰落在玩家已经看得
    /// 见的帧上。宿主显式要求预热时（game 宿主的 `--prewarm-particles`），这份钱挪到启动期。
    ///
    /// `0` = 没有可编的（设备没有存储缓冲 ⇒ 没有 GPU 粒子管线布局）；HDR 变体只在设备
    /// `hdr_usable()` 时编，对不支持的格式建管线会被 wgpu 校验拒绝。两种格式都缓存后，
    /// HDR/LDR 之间切换也不再重编。
    pub fn prewarm_gpu_particles(&mut self) -> u32 {
        if self.gpu_particle_pipeline_layout.is_none() {
            return 0;
        }
        let ldr = self.config.format;
        let mut formats = vec![ldr];
        if self.capabilities.hdr_usable() && ldr != wgpu::TextureFormat::Rgba16Float {
            formats.push(wgpu::TextureFormat::Rgba16Float);
        }
        let mut built = 0;
        for format in formats {
            let known = self
                .gpu_particle_pipelines
                .iter()
                .any(|(cached, _)| *cached == format);
            if !known {
                self.ensure_gpu_particle_pipeline(format);
                built += 1;
            }
        }
        built
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
        assert_eq!(std::mem::size_of::<GpuParticleParams>(), 240);
        assert_eq!(std::mem::size_of::<GpuGridMeta>(), 48);
    }

    /// P3-R234：网格变体必须真的从基段**派生**出来，且派生不是静默 no-op。
    ///
    /// `replace` 锚点失配时什么都不改——那样"网格版"会退化成基段，compute 照样跑、粒子
    /// 照样动，只是场永远不生效。这条门钉住三件事：变体含采样函数；把两处插入剥掉后
    /// 与基段**逐字节相等**（共享段不可能分叉）；基段本身不含网格（无场时默认路径不变）。
    #[test]
    fn gpu_grid_shader_derives_from_base() {
        let grid = gpu_particle_grid_shader();
        assert!(
            grid.contains("fn sample_grid_accel"),
            "网格变体没有采样函数：派生锚点失配（replace 静默 no-op）"
        );
        assert!(grid.contains("struct GridMeta"));
        assert!(grid.contains("var<storage, read> grid_data"));
        assert!(grid.contains("let grid_accel = sample_grid_accel"));
        assert!(
            grid.contains("    state.velocity = state.velocity + (params.gravity + wind) * dt;"),
            "网格变体丢了积分行"
        );
        let stripped = grid
            .replace(GPU_GRID_DECLS, "")
            .replace(GPU_GRID_SAMPLE, "");
        assert_eq!(stripped, GPU_PARTICLE_SHADER, "共享段与基段分叉了");
        assert!(
            !GPU_PARTICLE_SHADER.contains("grid_data"),
            "基段不该含网格：无场时必须逐位不变"
        );
    }

    #[test]
    fn gpu_flutter_is_packed_into_buoyancy_pad() {
        let emitter = ParticleEmitter {
            flutter: 1.5,
            flutter_hz: 2.0,
            buoyancy: 0.75,
            curve: 0.3,
            ..ParticleEmitter::default()
        };
        let params = Renderer::gpu_particle_params(
            &emitter,
            0.5,
            [0.0, 0.0],
            0.0,
            [0.0, 0.0, 0.0],
            [[0.0; 7]; 4],
            1.0,
        );
        assert_eq!(params.buoyancy_pad, [0.75, 1.5, 2.0, 0.3]);
    }

    #[test]
    fn gpu_curl_is_packed_into_curl_pad() {
        let emitter = ParticleEmitter {
            curl: 1.25,
            curl_scale: 0.4,
            curl_hz: 0.75,
            ..ParticleEmitter::default()
        };
        let params = Renderer::gpu_particle_params(
            &emitter,
            0.5,
            [0.0, 0.0],
            0.0,
            [0.0, 0.0, 0.0],
            [[0.0; 7]; 4],
            1.0,
        );
        assert_eq!(params.curl_pad, [1.25, 0.4, 0.75, 0.0]);
    }
    #[test]
    fn gpu_wind_is_packed_into_the_existing_uniform() {
        let emitter = ParticleEmitter::new(4, 10.0, 1.0, [1.0; 4]);
        let sources = [
            [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
            [0.0; 7],
            [0.0; 7],
            [0.0; 7],
        ];
        let params = Renderer::gpu_particle_params(
            &emitter,
            0.5,
            [2.5, -3.5],
            4.5,
            [0.2, 0.3, 0.4],
            sources,
            1.25,
        );
        assert_eq!(params.dt, 0.5);
        assert_eq!(params.time, 1.25);
        assert_eq!(params.wind_x, 2.5);
        assert_eq!(params.wind_y, -3.5);
        assert_eq!(params.wind_z, 4.5);
        assert_eq!(params.wind_shear, 0.2);
        assert_eq!(params.wind_gust, 0.3);
        assert_eq!(params.wind_gust_hz, 0.4);
        assert_eq!(params.source_a[0], [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(params.source_b[0], [5.0, 6.0, 1.0, 7.0]);
        assert_eq!(params.source_b[1][2], 0.0, "radius=0 is inactive");
        assert_eq!(params.gravity, [0.0, 0.0]);
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
        assert!(GPU_PARTICLE_DRAW_SHADER.contains("textureLoad(scene_depth"));
        assert!(GPU_PARTICLE_DRAW_SHADER.contains("smoothstep(0.0, 0.02, delta)"));
        assert_eq!(
            common.matches("struct ").count(),
            2,
            "公共段应当只有 ParticleState 与 Params 两个结构"
        );
    }
}
