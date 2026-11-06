struct Camera {
    view_projection: mat4x4<f32>,
    // xyz = 世界位置，w = 时间（秒）
    position: vec4<f32>,
    // xyz = 归一化平面法线，w = 偏移；零向量 = 不裁剪（会话 X-ray/剖面）
    clip: vec4<f32>,
};
struct ShadowCameras { view_projection: array<mat4x4<f32>, 4> };
struct PointShadow {
    cameras: array<mat4x4<f32>, 48>, // 8  × 6 面，槽位 s 的基底是 s*6
    params: array<vec4<f32>, 8>, // xyz = 点光世界坐标，w = 远平面；w=0 表示槽位未用
};
struct PointLight { position_range: vec4<f32>, color_intensity: vec4<f32> };
// 方向 48B + 8 盏点光（CPU  `light_uniform_bytes` 同布局）
struct Light {
    direction: vec4<f32>,
    color_intensity: vec4<f32>,
    // x = 点光数量 ..=8），y = 3×3 PCF 软阴影开关，其余保留
    params: vec4<f32>,
    points: array<PointLight, 8>,
};
struct Material {
    base_color: vec4<f32>,
    surface: vec4<f32>,
    emissive: vec4<f32>,
    params: vec4<f32>,
    flags: vec4<f32>,
    // x=1 内建水面（顶点涟 + 边缘软边）；x=2 内建地形（高 坡度配色）
    animation: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> light: Light;
@group(0) @binding(2) var<uniform> shadow_camera: ShadowCameras;
@group(0) @binding(3) var shadow_map: texture_depth_2d_array;
@group(0) @binding(4) var shadow_sampler: sampler_comparison;
@group(0) @binding(5) var<uniform> point_shadow: PointShadow;
@group(0) @binding(6) var point_shadow_cube: texture_depth_cube_array;
@group(0) @binding(7) var point_shadow_sampler: sampler_comparison;
@group(0) @binding(8) var esm_map: texture_2d_array<f32>;
@group(0) @binding(11) var point_shadow_plain_sampler: sampler;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var<uniform> joint_palette: array<mat4x4<f32>, 128>;
//#if STORAGE_INSTANCES
@group(1) @binding(2) var<storage, read> instance_models: array<mat4x4<f32>>;
//#else
// WebGL2 没有存储缓冲：每 draw 一个模型矩阵， 64 字节 uniform（实例化合批已关）
@group(1) @binding(2) var<uniform> instance_model: mat4x4<f32>;
//#endif
@group(1) @binding(3) var albedo: texture_2d<f32>;
@group(1) @binding(4) var albedo_sampler: sampler;
// P3-R02：glTF ORM 约定（G=粗糙度，B=金属度）与自发光。缺省贴图是 1x1 白，
// 因此"只有因子、没有贴 时采样退化为因子直通
@group(1) @binding(5) var metallic_roughness_map: texture_2d<f32>;
@group(1) @binding(6) var emissive_map: texture_2d<f32>;
// P3-R02-3：切线空间法线贴图与遮蔽贴图（线性数据）
@group(1) @binding(7) var normal_map: texture_2d<f32>;
@group(1) @binding(8) var occlusion_map: texture_2d<f32>;
// P3-R03：环境光照。SH9 系数已按 A_l/π 预卷积，`sh_irradiance` 直接返回 E/π
// IBL 关闭时这 uniform 仍然绑定（全 + intensity=0），避免切换管线布局
// params: x=intensity, y=rotation(弧度), z=mode code(1=SH, 2=spec, 3=both),
// w=预过 mip 的最 LOD（mip_count-1）
struct Environment {
    sh: array<vec4<f32>, 9>,
    params: vec4<f32>,
};
@group(2) @binding(0) var<uniform> environment: Environment;
// P3-R03-3：预过滤 cubemap（ mip GGX）。没有环境时绑定 1×1  cube
@group(2) @binding(1) var environment_cube: texture_cube<f32>;
@group(2) @binding(2) var environment_sampler: sampler;
// P3-R02：逐实例法线矩阵走顶点缓冲 1（location 6..9），不是存储绑定
// 逆转置在 CPU 侧算（WGSL 没有 `inverse` 内建）；放在顶点缓冲里是为了不增
// bind group 布局条目——多一个条目会显著拉长启动期管线编译
const PI: f32 = 3.141592653589793;
const DIELECTRIC_F0: f32 = 0.04;
const AMBIENT_FACTOR: f32 = 0.15;
const MIN_ROUGHNESS: f32 = 0.045;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) shadow_position: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) world_position: vec3<f32>,
    // w = 切线手性；w = 0 表示没有可用切线基，片元退回几何法线
    @location(4) tangent: vec4<f32>,
    // 内建地形专用：x=草、y=苔、z=落叶、w=裸土；其它网格原样透传蒙皮权重
    // 地形分支之外不读取。见 `terrain.rs::splat_weights`
    @location(5) splat: vec4<f32>,
    @location(6) shadow_position_mid: vec4<f32>,
    @location(7) shadow_position_far: vec4<f32>,
    @location(8) shadow_position_ultra: vec4<f32>,
};

@vertex fn vs_main(@builtin(instance_index) instance_index: u32, @location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(5) tangent: vec4<f32>, @location(3) joint_indices: vec4<u32>, @location(4) bone_weights: vec4<f32>, @location(6) normal_m0: vec4<f32>, @location(7) normal_m1: vec4<f32>, @location(8) normal_m2: vec4<f32>, @location(9) normal_m3: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
//#if STORAGE_INSTANCES
    let model = instance_models[instance_index];
//#else
    let model = instance_model;
//#endif
    let normal_matrix = mat4x4<f32>(normal_m0, normal_m1, normal_m2, normal_m3);
    var local_position = vec4<f32>(position, 1.0);
    var local_normal = vec4<f32>(normal, 0.0);
    var local_tangent = vec4<f32>(tangent.xyz, 0.0);
//#if SKIN
    var skinned_position = vec3<f32>(0.0);
    var skinned_normal = vec3<f32>(0.0);
    var skinned_tangent = vec3<f32>(0.0);
    skinned_position += (joint_palette[joint_indices.x] * local_position).xyz * bone_weights.x;
    skinned_position += (joint_palette[joint_indices.y] * local_position).xyz * bone_weights.y;
    skinned_position += (joint_palette[joint_indices.z] * local_position).xyz * bone_weights.z;
    skinned_position += (joint_palette[joint_indices.w] * local_position).xyz * bone_weights.w;
    skinned_normal += (joint_palette[joint_indices.x] * local_normal).xyz * bone_weights.x;
    skinned_normal += (joint_palette[joint_indices.y] * local_normal).xyz * bone_weights.y;
    skinned_normal += (joint_palette[joint_indices.z] * local_normal).xyz * bone_weights.z;
    skinned_normal += (joint_palette[joint_indices.w] * local_normal).xyz * bone_weights.w;
    skinned_tangent += (joint_palette[joint_indices.x] * local_tangent).xyz * bone_weights.x;
    skinned_tangent += (joint_palette[joint_indices.y] * local_tangent).xyz * bone_weights.y;
    skinned_tangent += (joint_palette[joint_indices.z] * local_tangent).xyz * bone_weights.z;
    skinned_tangent += (joint_palette[joint_indices.w] * local_tangent).xyz * bone_weights.w;
    local_position = vec4<f32>(skinned_position, 1.0);
    local_normal = vec4<f32>(skinned_normal, 0.0);
    local_tangent = vec4<f32>(skinned_tangent, 0.0);
//#endif
    let world_position = model * local_position;
    // 水面：几何本身已有低频高度场，这里再叠随时间移动的小涟漪。位 阴影/视线
    // 全部用抬升后的世界坐标，否则高光会和几何错位
    var shaded_world = world_position;
    if (material.animation.x > 0.5 && material.animation.x < 1.5) {
        let t = camera.position.w;
        let ripple = sin(world_position.x * 0.75 + t * 1.7)
            * cos(world_position.z * 0.60 - t * 1.3)
            + 0.5 * sin((world_position.x + world_position.z) * 0.35 + t * 2.2);
        shaded_world.y += ripple * 0.018;
    }
    out.position = camera.view_projection * shaded_world;
    out.normal = (normal_matrix * local_normal).xyz;
    out.shadow_position = shadow_camera.view_projection[0] * shaded_world;
    out.shadow_position_mid = shadow_camera.view_projection[1] * shaded_world;
    out.shadow_position_far = shadow_camera.view_projection[2] * shaded_world;
    out.shadow_position_ultra = shadow_camera.view_projection[3] * shaded_world;
    out.uv = uv;
    out.world_position = shaded_world.xyz;
    out.tangent = vec4<f32>((normal_matrix * local_tangent).xyz, tangent.w);
    out.splat = bone_weights;
    return out;
}

fn safe_normalize(value: vec3<f32>) -> vec3<f32> {
    let length_sq = dot(value, value);
    return select(vec3<f32>(0.0, 1.0, 0.0), value * inverseSqrt(length_sq), length_sq > 1e-12);
}

// 程序化地表颗粒：不用贴图，给地形底色加一点世界空间噪声，避免大片纯色
fn hash21(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(123.34, 456.21));
    let r = q + dot(q, q + vec2<f32>(45.32));
    return fract(r.x * r.y);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// 程序化云影：用相机时间做缓慢漂移，给整片地面/水面一层大尺度明暗。
// 只压 base color（地形无高光），不参与阴影贴图，因此和现有阴影叠加很便宜。
fn cloud_shadow(world: vec2<f32>, time: f32) -> f32 {
    let drift = vec2<f32>(time * 0.35, time * 0.12);
    let p = world * 0.0035 + drift * 0.05;
    let n = value_noise(p) * 0.62 + value_noise(p * 2.3 + vec2<f32>(7.1, 3.7)) * 0.38;
    return smoothstep(0.50, 0.74, n);
}

// 地形细节图集（`terrain_detail.rs` 生成、随材质内嵌）：2x2 = 草/苔/落叶/裸土。
// `tile` 取 (0|1, 0|1)，`local` = fract(世界坐标 / tile 米数)。
// 半纹素内缩：lod 0 时线性过滤不会把相邻层擦进来；mip 由生成期上传，lod 上限 5。
fn terrain_atlas_sample(local: vec2<f32>, tile: vec2<f32>, lod: f32) -> vec3<f32> {
    let inset = 1.0 / 512.0;
    let clamped = clamp(local, vec2<f32>(inset), vec2<f32>(1.0 - inset));
    return textureSampleLevel(albedo, albedo_sampler, (tile + clamped) * 0.5, lod).rgb;
}

// 地形法线图集（由颜色图集的亮度求梯度生成，2x2 与颜色图集同布局）。
// R = n.x、G = n.z、B = n.y（线性 RGBA8）；片元只用 RG 重建 XZ 斜率。
fn terrain_normal_atlas(local: vec2<f32>, tile: vec2<f32>, lod: f32) -> vec3<f32> {
    let inset = 1.0 / 512.0;
    let clamped = clamp(local, vec2<f32>(inset), vec2<f32>(1.0 - inset));
    return textureSampleLevel(normal_map, albedo_sampler, (tile + clamped) * 0.5, lod).rgb
        * 2.0 - vec3<f32>(1.0);
}

// Trowbridge-Reitz（GGX）法线分布。`alpha` = roughness²
fn distribution_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-7);
}

// Smith height-correlated visibility（已并入 1/(4 NdotL NdotV)）
fn visibility_smith_correlated(n_dot_v: f32, n_dot_l: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-7);
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    let f = pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
    return f0 + (vec3<f32>(1.0) - f0) * f;
}

//  Y 轴旋转（标准右手系）。环境绕 +Y  θ 等价于采样时把方向转 -θ
// 漫反射与镜面共用
fn rotate_y(direction: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3<f32>(c * direction.x + s * direction.z, direction.y, -s * direction.x + c * direction.z);
}

//#if IBL_DIFFUSE
// SH9 辐照度。基函数 `rustlua_assets::env_asset::sh_irradiance` 同一套常数，
// 系数已在烘焙期乘 cosine 卷积因子，因此返回值已经是 E/π
fn sh_irradiance(direction: vec3<f32>) -> vec3<f32> {
    let x = direction.x;
    let y = direction.y;
    let z = direction.z;
    var result = environment.sh[0].rgb * 0.282095;
    result = result + environment.sh[1].rgb * (0.488603 * y);
    result = result + environment.sh[2].rgb * (0.488603 * z);
    result = result + environment.sh[3].rgb * (0.488603 * x);
    result = result + environment.sh[4].rgb * (1.092548 * x * y);
    result = result + environment.sh[5].rgb * (1.092548 * y * z);
    result = result + environment.sh[6].rgb * (0.315392 * (3.0 * z * z - 1.0));
    result = result + environment.sh[7].rgb * (1.092548 * x * z);
    result = result + environment.sh[8].rgb * (0.546274 * (x * x - y * y));
    return max(result, vec3<f32>(0.0));
}
//#endif

//#if IBL_SPECULAR
// Karis  EnvBRDFApprox（移动端近似）：给定 F0/roughness/N·V 直接得到
// 预过滤环境的 BRDF 权重，不依赖 LUT。与 `host::renderer::pbr::env_brdf_approx` 同式
fn env_brdf_approx(f0: vec3<f32>, roughness: f32, n_dot_v: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * n_dot_v)) * r.x + r.y;
    let env = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return max(f0 * env.x + env.y, vec3<f32>(0.0));
}

// 反射方向采样预过 cubemap：lod = roughness * (mip_count - 1)
// 环境旋转同样通过把反射方向转 -θ 实现
fn sample_environment_specular(normal: vec3<f32>, view: vec3<f32>, roughness: f32) -> vec3<f32> {
    let reflection = rotate_y(reflect(-view, normal), -environment.params.y);
    let lod = clamp(roughness, 0.0, 1.0) * environment.params.w;
    return textureSampleLevel(environment_cube, environment_sampler, reflection, lod).rgb;
}
//#endif

// 方向光阴影：默认单次硬件比较；`light.params.y` 打开后做 PCSS-lite
// 先在固定 5×5 邻域找遮挡体平均深度，再按「受光深  遮挡体深度」放 PCF 半径
// 得到贴地硬、离地软的联系硬化。半径是纹素域近似，不做光源物理尺寸换算
fn sample_esm(shadow_uv: vec2<f32>, shadow_depth: f32, layer: i32) -> f32 {
    let dimensions = vec2<i32>(textureDimensions(esm_map));
    let last = max(dimensions - vec2<i32>(1, 1), vec2<i32>(0, 0));
    let base = vec2<i32>(clamp(
        shadow_uv * vec2<f32>(dimensions),
        vec2<f32>(0.0),
        vec2<f32>(last),
    ));
    var sum = 0.0;
    for (var y: i32 = -1; y <= 1; y = y + 1) {
        for (var x: i32 = -1; x <= 1; x = x + 1) {
            let coord = clamp(base + vec2<i32>(x, y), vec2<i32>(0, 0), last);
            sum = sum + textureLoad(esm_map, coord, layer, 0).r;
        }
    }
    // 9 个 exp 的平均 × exp(-receiver)：occluder 越近，比值越小、阴影越深。
    return clamp(exp(-10.0 * (shadow_depth - 0.002)) * (sum / 9.0), 0.0, 1.0);
}

fn sample_shadow(shadow_uv: vec2<f32>, shadow_depth: f32, layer: i32) -> f32 {
    if (shadow_uv.x < 0.0 || shadow_uv.x > 1.0 || shadow_uv.y < 0.0 || shadow_uv.y > 1.0) {
        return 1.0;
    }
    if (light.params.z > 0.5) {
        return sample_esm(shadow_uv, shadow_depth, layer);
    }
    if (light.params.y < 0.5) {
        return textureSampleCompare(shadow_map, shadow_sampler, shadow_uv, layer, shadow_depth);
    }
    let dimensions = vec2<i32>(textureDimensions(shadow_map));
    let last = max(dimensions - vec2<i32>(1, 1), vec2<i32>(0, 0));
    let base = vec2<i32>(clamp(
        shadow_uv * vec2<f32>(dimensions),
        vec2<f32>(0.0),
        vec2<f32>(last),
    ));
    var blocker_sum = 0.0;
    var blocker_count = 0;
    for (var y: i32 = -2; y <= 2; y = y + 1) {
        for (var x: i32 = -2; x <= 2; x = x + 1) {
            let coord = base + vec2<i32>(x, y);
            if coord.x < 0 || coord.y < 0 || coord.x > last.x || coord.y > last.y {
                continue;
            }
            let stored = textureLoad(shadow_map, coord, layer, 0);
            if stored < shadow_depth - 0.0005 {
                blocker_sum = blocker_sum + stored;
                blocker_count = blocker_count + 1;
            }
        }
    }
    if blocker_count == 0 {
        return 1.0;
    }
    let average_blocker = blocker_sum / f32(blocker_count);
    let radius = clamp((shadow_depth - average_blocker) * 48.0, 1.0, 3.0);
    let texel = 1.0 / vec2<f32>(dimensions);
    var sum = 0.0;
    for (var y: i32 = -2; y <= 2; y = y + 1) {
        for (var x: i32 = -2; x <= 2; x = x + 1) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel * (radius * 0.5);
            sum = sum + textureSampleCompare(shadow_map, shadow_sampler, shadow_uv + offset, layer, shadow_depth);
        }
    }
    return sum / 25.0;
}

// 把一级的裁剪坐标压成 `(uv, depth, inside)`；inside  1/0 便于分支
fn cascade_coords(clip: vec4<f32>) -> vec4<f32> {
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 1.0 - (ndc.y * 0.5 + 0.5));
    let inside = ndc.z >= 0.0 && ndc.z <= 1.0 &&
        uv.x >= 0.0 && uv.x <= 1.0 &&
        uv.y >= 0.0 && uv.y <= 1.0;
    return vec4<f32>(uv, ndc.z, select(0.0, 1.0, inside));
}

// 到本级的归一化边界距离：0=中心 =裁剪体边缘
fn cascade_edge(uv: vec2<f32>) -> f32 {
    return max(abs(uv.x * 2.0 - 1.0), abs(uv.y * 2.0 - 1.0));
}

// 四级阴影：近→中→远→超远依次尝试；每级最 14% 的边界带与下一级混合，
// 消除级联接缝。中心区域仍只采样一级，不付双倍阴影开销
fn shadow_visibility_four_cascades(
    near_clip: vec4<f32>,
    mid_clip: vec4<f32>,
    far_clip: vec4<f32>,
    ultra_clip: vec4<f32>,
) -> f32 {
    let near = cascade_coords(near_clip);
    if near.w > 0.5 {
        let visible = sample_shadow(near.xy, near.z - 0.002, 0);
        let edge = cascade_edge(near.xy);
        if edge > 0.86 {
            let mid = cascade_coords(mid_clip);
            if mid.w > 0.5 {
                return mix(
                    visible,
                    sample_shadow(mid.xy, mid.z - 0.002, 1),
                    smoothstep(0.86, 1.0, edge),
                );
            }
        }
        return visible;
    }
    let mid = cascade_coords(mid_clip);
    if mid.w > 0.5 {
        let visible = sample_shadow(mid.xy, mid.z - 0.002, 1);
        let edge = cascade_edge(mid.xy);
        if edge > 0.86 {
            let far = cascade_coords(far_clip);
            if far.w > 0.5 {
                return mix(
                    visible,
                    sample_shadow(far.xy, far.z - 0.002, 2),
                    smoothstep(0.86, 1.0, edge),
                );
            }
        }
        return visible;
    }
    let far = cascade_coords(far_clip);
    if far.w > 0.5 {
        let visible = sample_shadow(far.xy, far.z - 0.002, 2);
        let edge = cascade_edge(far.xy);
        if edge > 0.86 {
            let ultra = cascade_coords(ultra_clip);
            if ultra.w > 0.5 {
                return mix(
                    visible,
                    sample_shadow(ultra.xy, ultra.z - 0.002, 3),
                    smoothstep(0.86, 1.0, edge),
                );
            }
        }
        return visible;
    }
    let ultra = cascade_coords(ultra_clip);
    if ultra.w > 0.5 {
        return sample_shadow(ultra.xy, ultra.z - 0.002, 3);
    }
    return 1.0;
}

// 点光阴影采样：voxel cube array 让硬件在 6 个面之间做跨面过滤，消除手工
// 选面 + UV clamp 在面边界留下的接缝。软阴影仍是 3×3 方向扰动 PCF。
fn sample_point_shadow(direction: vec3<f32>, slot: i32, shadow_depth: f32) -> f32 {
    let dir = normalize(direction);
    if (light.params.y < 0.5) {
        return textureSampleCompare(
            point_shadow_cube,
            point_shadow_sampler,
            dir,
            slot,
            shadow_depth,
        );
    }
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(dir.y) > 0.9);
    let tangent = normalize(cross(up, dir));
    let bitangent = cross(dir, tangent);
    // PCSS-lite：先在 5×5 采遮挡体平均深度，再按接触距离放大 PCF 半径。
    var blocker_sum = 0.0;
    var blocker_count = 0.0;
    for (var y: i32 = -2; y <= 2; y = y + 1) {
        for (var x: i32 = -2; x <= 2; x = x + 1) {
            let offset = tangent * (f32(x) * 0.006) + bitangent * (f32(y) * 0.006);
            let stored = textureSample(
                point_shadow_cube,
                point_shadow_plain_sampler,
                normalize(dir + offset),
                slot,
            );
            if stored < shadow_depth - 0.0005 {
                blocker_sum = blocker_sum + stored;
                blocker_count = blocker_count + 1.0;
            }
        }
    }
    if blocker_count < 0.5 {
        return 1.0;
    }
    let radius = clamp((shadow_depth - blocker_sum / blocker_count) * 48.0, 1.0, 3.0);
    var sum = 0.0;
    for (var y: i32 = -2; y <= 2; y = y + 1) {
        for (var x: i32 = -2; x <= 2; x = x + 1) {
            let offset = tangent * (f32(x) * 0.004 * radius)
                + bitangent * (f32(y) * 0.004 * radius);
            sum = sum + textureSampleCompare(
                point_shadow_cube,
                point_shadow_sampler,
                normalize(dir + offset),
                slot,
                shadow_depth,
            );
        }
    }
    return sum / 25.0;
}

// 点光槽位按位置匹配：CPU 只给前 8 盏写阴影矩阵，槽位未用时 params.w=0
fn point_shadow_slot(light_position: vec3<f32>) -> i32 {
    for (var slot = 0u; slot < 8u; slot = slot + 1u) {
        let params = point_shadow.params[slot];
        if (params.w > 0.5 && distance(light_position, params.xyz) < 1e-3) {
            return i32(slot);
        }
    }
    return -1;
}

// 每盏点光的六面阴影：把世界点到光源的方向直接交给 cube array 采样，
// 硬件按最大轴选面并在面边界过滤；不再由 CPU 侧手工选面/拼 UV。
fn point_shadow_visibility(world_position: vec3<f32>, slot: u32) -> f32 {
    let params = point_shadow.params[slot];
    let light_position = params.xyz;
    let delta = world_position - light_position;
    let distance_to_light = length(delta);
    if (distance_to_light < 1e-3 || distance_to_light > params.w) {
        return 1.0;
    }
    // 参考深度必须用点实际落在的那一面投影；面选择由 cube 方向完成。
    let a = abs(delta);
    var face = 0;
    if (a.x >= a.y && a.x >= a.z) {
        face = select(0, 1, delta.x < 0.0);
    } else if (a.y >= a.z) {
        face = select(2, 3, delta.y < 0.0);
    } else {
        face = select(4, 5, delta.z < 0.0);
    }
    let clip = point_shadow.cameras[i32(slot) * 6 + face] * vec4<f32>(world_position, 1.0);
    let depth_ref = clamp(clip.z / clip.w - 0.004, 0.0, 1.0);
    return sample_point_shadow(delta, i32(slot), depth_ref);
}

@fragment fn fs_main(@location(0) normal: vec3<f32>, @location(1) shadow_position: vec4<f32>, @location(2) uv: vec2<f32>, @location(3) world_position: vec3<f32>, @location(4) tangent: vec4<f32>, @location(5) splat: vec4<f32>, @location(6) shadow_position_mid: vec4<f32>, @location(7) shadow_position_far: vec4<f32>, @location(8) shadow_position_ultra: vec4<f32>, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    let albedo_color = textureSample(albedo, albedo_sampler, uv);
    var base_rgb = material.base_color.rgb * albedo_color.rgb;
    var base_alpha = material.base_color.a * albedo_color.a;
    // 会话级世界空间裁剪平面（X-ray/剖面）。零向量显式跳过，保证旧路径像素不变
    if (dot(camera.clip.xyz, camera.clip.xyz) > 1e-8
        && dot(world_position, camera.clip.xyz) + camera.clip.w < 0.0) {
        discard;
    }
    // P3-R02-4：flags.z = 0/1/2（opaque/mask/blend），flags.w = cutoff
    // opaque 强制 alpha=1；mask 低于阈 discard；blend 保留 alpha 交给管线混合
    if (material.flags.z < 0.5) {
        base_alpha = 1.0;
    }
    if (material.flags.z > 0.5 && material.flags.z < 1.5 && base_alpha < material.flags.w) {
        discard;
    }
    // 地形
    // 通道 splat（草/苔/落叶/裸土）先混出植被底色，再用高度把
    // 河床压回湿沙、用坡度压向岩石。weights 来自非蒙皮地形的蒙皮权重通道。
    // `surface.w > 0` 时（`detail=<世界单位/瓦片>`）改用内嵌的 2x2 程序化贴图图集，
    // 按世界坐标采样、按顶点权重混合，并按纹素足迹选 mip；
    // 否则保持 100% 原来的纯色路径（旧像素逐字节不变）。
    // uv.x = 归一化高度，uv.y = 1 - normal.y
    var terrain_normal = vec2<f32>(0.0, 0.0);
    if (material.animation.x > 1.5) {
        let height_t = clamp(uv.x, 0.0, 1.0);
        let slope_t = clamp(uv.y, 0.0, 1.0);
        let sand = vec3<f32>(0.52, 0.44, 0.30);
        let soil = vec3<f32>(0.32, 0.28, 0.20);
        let rock = vec3<f32>(0.38, 0.35, 0.31);
        let weights = clamp(splat, vec4<f32>(0.0), vec4<f32>(1.0));
        var bare: vec3<f32>;
        if (material.surface.w > 0.001) {
            let tile_scale = max(material.surface.w, 0.05);            let p = world_position.xz / tile_scale;
            let local = fract(p);
            // 图集 UV 每像素变化 = dp(p) * 0.5；乘图集宽度得到纹素足迹。
            let step_x = length(dpdx(p)) * 0.5;
            let step_y = length(dpdy(p)) * 0.5;
            let atlas_width = f32(textureDimensions(albedo).x);
            let lod = clamp(log2(max(step_x, step_y) * atlas_width), 0.0, 3.0);
            let grass_tex = terrain_atlas_sample(local, vec2<f32>(0.0, 0.0), lod);
            let moss_tex = terrain_atlas_sample(local, vec2<f32>(1.0, 0.0), lod);
            let litter_tex = terrain_atlas_sample(local, vec2<f32>(0.0, 1.0), lod);
            let soil_tex = terrain_atlas_sample(local, vec2<f32>(1.0, 1.0), lod);
            let vegetation = grass_tex * weights.x + moss_tex * weights.y
                + litter_tex * weights.z;
            bare = mix(vegetation, soil_tex, weights.w);
            let n0 = terrain_normal_atlas(local, vec2<f32>(0.0, 0.0), lod);
            let n1 = terrain_normal_atlas(local, vec2<f32>(1.0, 0.0), lod);
            let n2 = terrain_normal_atlas(local, vec2<f32>(0.0, 1.0), lod);
            let n3 = terrain_normal_atlas(local, vec2<f32>(1.0, 1.0), lod);
            terrain_normal = vec2<f32>(
                n0.x * weights.x + n1.x * weights.y + n2.x * weights.z + n3.x * weights.w,
                n0.y * weights.x + n1.y * weights.y + n2.y * weights.z + n3.y * weights.w
            );
        } else {
            let grass = vec3<f32>(0.20, 0.31, 0.15);
            let moss = vec3<f32>(0.15, 0.25, 0.13);
            let litter = vec3<f32>(0.30, 0.21, 0.11);
            let vegetation = grass * weights.x + moss * weights.y + litter * weights.z;
            bare = mix(vegetation, soil, weights.w);
        }
        var ground = mix(sand, bare, smoothstep(0.04, 0.24, height_t));
        // 三档世界空间颗粒：低频斑 + 中频碎屑 + 高频沙粒；只压亮度，
        // 不改 splat 权重，所以和  落叶的配色不打架
        let grain_pos = world_position.xz;
        // 大尺度明暗斑：远景（mip 已经平均掉细节）仍能看出地表起伏和湿度差，
        // 而不是一整片同色。频率按世界单位，和贴图瓦片无关。
        let macro_light = value_noise(grain_pos * 0.045) * 0.65
            + value_noise(grain_pos * 0.12) * 0.35;
        ground *= 0.84 + macro_light * 0.32;
        let grain = value_noise(grain_pos * 1.15) * 0.55
            + value_noise(grain_pos * 4.70) * 0.30
            + value_noise(grain_pos * 13.0) * 0.15;
        ground *= 1.0 + (grain - 0.5) * 0.30;
        ground = mix(ground, rock, smoothstep(0.28, 0.72, slope_t));
        // 陡坡上的岩石也加一点纵向擦痕，避免整片坡一个色
        let streak = value_noise(vec2<f32>(world_position.x * 0.35, world_position.z * 2.6));
        ground = mix(ground, ground * vec3<f32>(0.90, 0.91, 0.94),
            smoothstep(0.55, 0.86, streak) * smoothstep(0.28, 0.72, slope_t) * 0.45);
        // flags.w carries the normalized shore level for terrain (opaque, so
        // the alpha-cutoff slot is unused otherwise). 1.0 = no shore band.
        if (material.flags.w < 0.999) {
            let shore_t = clamp(material.flags.w, 0.01, 0.98);
            let below = 1.0 - smoothstep(shore_t, shore_t + 0.04, height_t);
            ground = mix(ground, ground * vec3<f32>(0.58, 0.64, 0.62), below * 0.65);
        }
        base_rgb = ground;
        let cloud = cloud_shadow(world_position.xz, camera.position.w);
        base_rgb = mix(base_rgb, base_rgb * vec3<f32>(0.72, 0.78, 0.86), cloud * 0.65);
    }
    // 水面：有 bed 的水面在顶点里烘 depth（splat.x，splat.y=1 标记），
    // 因而可以按真实河床做深浅色/岸线；没 bed 的老水面仍 UV 边缘软边
    var water_wetness = 1.0;
    if (material.animation.x > 0.5 && material.animation.x < 1.5) {
        let has_bed = splat.y > 0.5;
        // 有 bed 时优先读逐像素河床深度图（normal 槽位、线性上传，`r*9-1`）；
        // 没有深度图/老水面时退回逐顶点 splat.x。
        var signed_depth = splat.x;
        var depth = max(splat.x, 0.0);
        if (has_bed && material.surface.w < -0.5) {
            signed_depth = textureSampleLevel(normal_map, albedo_sampler, uv, 0.0).r * 9.0 - 1.0;
            depth = max(signed_depth, 0.0);
        }
        if (has_bed) {
            let deep = smoothstep(0.0, 2.0, depth);
            base_rgb = mix(vec3<f32>(0.14, 0.36, 0.40), vec3<f32>(0.03, 0.10, 0.17), deep);
            base_alpha = mix(0.48, 0.86, deep);
            // 浅水白沫：只在水 0..0.30 的窄带里出现
            let shore = 1.0 - smoothstep(0.02, 0.30, depth);
            base_rgb = mix(base_rgb, vec3<f32>(0.78, 0.86, 0.84), shore * 0.55);
            base_alpha = max(base_alpha, shore * 0.55);
            // 云影同样落在水面上，避免河流在云影里“自己发光”。
            let cloud = cloud_shadow(world_position.xz, camera.position.w);
            base_rgb = mix(base_rgb, base_rgb * vec3<f32>(0.74, 0.80, 0.88), cloud * 0.55);
            // 陆地/水面相交处按深度淡出 0.03 留一点抖动余量
            water_wetness = smoothstep(-0.03, 0.12, signed_depth);
        }
        let edge = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
        // 有 bed 的水面靠水深淡出岸线；UV 边缘再淡出会在相邻水块之间画出方格。
        base_alpha *= select(smoothstep(0.0, 0.10, edge), 1.0, has_bed) * water_wetness;
        // 岸线泡沫：有 bed 时按水深；没 bed 时退 UV 边缘 + 时间波纹
        // 强度 `WaterSpec.foam`  flags.w 带到材质
        let foam_strength = clamp(material.flags.w, 0.0, 1.0);
        if (foam_strength > 0.001) {
            let band = 1.0 - smoothstep(0.03, 0.16, edge);
            // flags.y 在水面材质上是沿局 +x 的流速（不是遮蔽强度）；负值倒流
            let flow = clamp(material.flags.y, -4.0, 4.0);
            let wave = 0.5 + 0.5 * sin(
                uv.x * 37.0 + uv.y * 23.0 + camera.position.w * 2.1
                - camera.position.w * flow * 1.4
            );
            let foam = clamp(band * foam_strength * (0.45 + 0.55 * wave), 0.0, 1.0)
                * water_wetness;
            base_rgb = mix(base_rgb, vec3<f32>(0.80, 0.87, 0.86), foam * 0.80);
            base_alpha = max(base_alpha, foam * 0.90);
        }
    }
    var geometric_normal = safe_normalize(normal);
    // 双面材质没有背面剔除，背面的几何法线要翻转才能得到正确的光照方向
    if (!front_facing) {
        geometric_normal = -geometric_normal;
    }
    var n = geometric_normal;
    // 地形：贴图法线按世界坐标/四层权重混合后直接进入 PBR 光照。它走地形分支
    // 自己的采样，不复用下面的切线基法线贴图路径。
    if (material.animation.x > 1.5 && material.surface.w > 0.001) {
        n = safe_normalize(vec3<f32>(terrain_normal.x * 2.0, 1.0, terrain_normal.y * 2.0));
    }
    // 水面：世界空间程序化波法线 + 菲涅尔。几何仍是平面，但法线参与直射高光、
    // IBL 反射和菲涅尔朝向；因为按世界坐标算，相邻水块之间没有相位接缝。
    if (material.animation.x > 0.5 && material.animation.x < 1.5) {
        let water_t = camera.position.w;
        let flow = clamp(material.flags.y, -4.0, 4.0);
        let advance = water_t * (0.9 + flow * 0.35);
        let p = world_position.xz;
        let k1 = vec2<f32>(0.62, 0.31);
        let k2 = vec2<f32>(-0.28, 0.74);
        let k3 = vec2<f32>(1.35, -0.85);
        let ph1 = dot(p, k1) + advance * 1.3;
        let ph2 = dot(p, k2) - advance * 1.7;
        let ph3 = dot(p, k3) + advance * 2.4;
        let slope = vec2<f32>(
            cos(ph1) * k1.x * 0.55 + cos(ph2) * k2.x * 0.35 + cos(ph3) * k3.x * 0.16,
            cos(ph1) * k1.y * 0.55 + cos(ph2) * k2.y * 0.35 + cos(ph3) * k3.y * 0.16
        );
        n = safe_normalize(vec3<f32>(-slope.x * 0.16, 1.0, -slope.y * 0.16));
        let view_dir = safe_normalize(camera.position.xyz - world_position);
        let fresnel = pow(1.0 - clamp(dot(n, view_dir), 0.0, 1.0), 5.0);
        base_rgb = mix(base_rgb, vec3<f32>(0.30, 0.42, 0.54), fresnel * 0.35);
        base_alpha = max(base_alpha, fresnel * 0.50);
    }
    // 法线贴图只在有贴图且切线基可用（w != 0）时启用；UV 退 镜像
    // CPU 侧把 w  0，这里自然退回几何法线，不再硬算错误切线
    if (material.animation.x < 0.5 && material.params.z > 0.5 && tangent.w != 0.0 && dot(tangent.xyz, tangent.xyz) > 1e-12) {
        let t = safe_normalize(tangent.xyz);
        let b = cross(geometric_normal, t) * tangent.w;
        let sampled = textureSample(normal_map, albedo_sampler, uv).rgb * 2.0 - vec3<f32>(1.0);
        let scaled = vec3<f32>(sampled.x * material.params.w, sampled.y * material.params.w, sampled.z);
        n = safe_normalize(t * scaled.x + b * scaled.y + geometric_normal * scaled.z);
    }
    let l = safe_normalize(-light.direction.xyz);
    let v = safe_normalize(camera.position.xyz - world_position);
    let h = safe_normalize(l + v);
    let n_dot_l = max(dot(n, l), 0.0);
    let n_dot_v = max(dot(n, v), 0.0);
    let n_dot_h = max(dot(n, h), 0.0);
    let v_dot_h = max(dot(v, h), 0.0);
    let metallic_roughness = textureSample(metallic_roughness_map, albedo_sampler, uv);
    let roughness = clamp(material.surface.y * metallic_roughness.g, MIN_ROUGHNESS, 1.0);
    let metallic = clamp(material.surface.x * metallic_roughness.b, 0.0, 1.0);
    let alpha = roughness * roughness;
    let f0 = mix(vec3<f32>(DIELECTRIC_F0), base_rgb, vec3<f32>(metallic));

    var direct = vec3<f32>(0.0);
    if (n_dot_l > 0.0 && n_dot_v > 0.0) {
        let fresnel = fresnel_schlick(v_dot_h, f0);
        let specular = distribution_ggx(n_dot_h, alpha) * visibility_smith_correlated(n_dot_v, n_dot_l, alpha) * fresnel;
        let kd = (vec3<f32>(1.0) - fresnel) * (1.0 - metallic);
        let diffuse = kd * base_rgb / PI;
        direct = (diffuse + specular) * n_dot_l;
    }

    // 点光源：同一 BRDF，衰减用"range 内平方反 × 平滑截断"
    // 前八盏点光可乘六面阴影可见度；其余点光只做直接光
    var point_direct = vec3<f32>(0.0);
    let point_count = min(u32(light.params.x), 8u);
    for (var i = 0u; i < point_count; i = i + 1u) {
        let point = light.points[i];
        let to_light = point.position_range.xyz - world_position;
        let distance = length(to_light);
        if (distance < 1e-4) {
            continue;
        }
        let point_l = to_light / distance;
        let point_n_dot_l = max(dot(n, point_l), 0.0);
        if (point_n_dot_l <= 0.0 || n_dot_v <= 0.0) {
            continue;
        }
        let point_h = safe_normalize(point_l + v);
        let point_fresnel = fresnel_schlick(max(dot(v, point_h), 0.0), f0);
        let point_specular = distribution_ggx(max(dot(n, point_h), 0.0), alpha) * visibility_smith_correlated(n_dot_v, point_n_dot_l, alpha) * point_fresnel;
        let point_kd = (vec3<f32>(1.0) - point_fresnel) * (1.0 - metallic);
        let point_diffuse = point_kd * base_rgb / PI;
        let falloff = clamp(1.0 - distance / max(point.position_range.w, 1e-4), 0.0, 1.0);
        let attenuation = falloff * falloff / (distance * distance + 1e-4);
        var point_visibility = 1.0;
        let shadow_slot = point_shadow_slot(point.position_range.xyz);
        if (shadow_slot >= 0) {
            point_visibility = point_shadow_visibility(world_position, u32(shadow_slot));
        }
        point_direct = point_direct + (point_diffuse + point_specular) * point_n_dot_l * attenuation * point.color_intensity.rgb * point.color_intensity.w * point_visibility;
    }

    let shadow_visibility = shadow_visibility_four_cascades(
        shadow_position,
        shadow_position_mid,
        shadow_position_far,
        shadow_position_ultra,
    );

    let radiance = light.color_intensity.rgb * light.color_intensity.w;
    // 环境遮蔽只作用在间接项（当前是常数环境光）；直接光与阴影沿用 glTF 语义
    var occlusion = 1.0;
    if (material.flags.x > 0.5) {
        let ao = textureSample(occlusion_map, albedo_sampler, uv).r;
        occlusion = mix(1.0, ao, clamp(material.flags.y, 0.0, 1.0));
    }
//#if IBL_DIFFUSE
    // P3-R03-2：SH9 漫反射。系数含 cosine 卷积，直接乘反照率；金属没有漫反射
    var indirect = base_rgb * sh_irradiance(rotate_y(n, -environment.params.y)) * environment.params.x * (1.0 - metallic) * occlusion;
//#else
    //#if IBL_SPECULAR
    // 只有镜面（显式档）：不保留常数环境漫反射，避免和镜面积分重复计入能量
    var indirect = vec3<f32>(0.0);
    //#else
    // IBL 关闭：保 R02-1 的常数环境光，回滚档像素 R02 一致；与旧路径一样不受阴影影响
    var indirect = base_rgb * AMBIENT_FACTOR * radiance * occlusion;
    //#endif
//#endif
//#if IBL_SPECULAR
    // P3-R03-3：预过滤镜面 IBL。EnvBRDFApprox 给出 F0/roughness/N·V 的权重，
    // mip  roughness 线性映射；遮蔽只作用在间接项
    indirect = indirect + sample_environment_specular(n, v, roughness) * env_brdf_approx(f0, roughness, n_dot_v) * environment.params.x * occlusion;
//#endif
    let emissive = material.emissive.rgb * textureSample(emissive_map, albedo_sampler, uv).rgb;
    return vec4<f32>(direct * radiance * shadow_visibility + point_direct + indirect + emissive, base_alpha);
}
