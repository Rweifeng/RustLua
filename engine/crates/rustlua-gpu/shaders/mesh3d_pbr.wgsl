struct Camera { view_projection: mat4x4<f32>, position: vec4<f32> };
struct ShadowCamera { view_projection: mat4x4<f32> };
struct Light { direction: vec4<f32>, color_intensity: vec4<f32> };
struct Material {
    base_color: vec4<f32>,
    surface: vec4<f32>,
    emissive: vec4<f32>,
    params: vec4<f32>,
    flags: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> light: Light;
@group(0) @binding(2) var<uniform> shadow_camera: ShadowCamera;
@group(0) @binding(3) var shadow_map: texture_depth_2d;
@group(0) @binding(4) var shadow_sampler: sampler_comparison;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var<uniform> joint_palette: array<mat4x4<f32>, 128>;
@group(1) @binding(2) var<storage, read> instance_models: array<mat4x4<f32>>;
@group(1) @binding(3) var albedo: texture_2d<f32>;
@group(1) @binding(4) var albedo_sampler: sampler;
// P3-R02：glTF ORM 约定（G=粗糙度，B=金属度）与自发光。缺省贴图是 1x1 白，
// 因此"只有因子、没有贴图"时采样退化为因子直通。
@group(1) @binding(5) var metallic_roughness_map: texture_2d<f32>;
@group(1) @binding(6) var emissive_map: texture_2d<f32>;
// P3-R02-3：切线空间法线贴图与遮蔽贴图（线性数据）。
@group(1) @binding(7) var normal_map: texture_2d<f32>;
@group(1) @binding(8) var occlusion_map: texture_2d<f32>;
// P3-R03：环境光照。SH9 系数已按 A_l/π 预卷积，`sh_irradiance` 直接返回 E/π。
// IBL 关闭时这份 uniform 仍然绑定（全零 + intensity=0），避免切换管线布局。
// params: x=intensity, y=rotation(弧度), z=mode code(1=SH, 2=spec, 3=both),
// w=预过滤 mip 的最大 LOD（mip_count-1）。
struct Environment {
    sh: array<vec4<f32>, 9>,
    params: vec4<f32>,
};
@group(2) @binding(0) var<uniform> environment: Environment;
// P3-R03-3：预过滤 cubemap（逐 mip GGX）。没有环境时绑定 1×1 黑 cube。
@group(2) @binding(1) var environment_cube: texture_cube<f32>;
@group(2) @binding(2) var environment_sampler: sampler;
// P3-R02：逐实例法线矩阵走顶点缓冲槽 1（location 6..9），不是存储绑定。
// 逆转置在 CPU 侧算（WGSL 没有 `inverse` 内建）；放在顶点缓冲里是为了不增加
// bind group 布局条目——多一个条目会显著拉长启动期管线编译。

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
    // w = 切线手性；w = 0 表示没有可用切线基，片元退回几何法线。
    @location(4) tangent: vec4<f32>,
};

@vertex fn vs_main(@builtin(instance_index) instance_index: u32, @location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(5) tangent: vec4<f32>, @location(3) joint_indices: vec4<u32>, @location(4) bone_weights: vec4<f32>, @location(6) normal_m0: vec4<f32>, @location(7) normal_m1: vec4<f32>, @location(8) normal_m2: vec4<f32>, @location(9) normal_m3: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
    let model = instance_models[instance_index];
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
    out.position = camera.view_projection * world_position;
    out.normal = (normal_matrix * local_normal).xyz;
    out.shadow_position = shadow_camera.view_projection * world_position;
    out.uv = uv;
    out.world_position = world_position.xyz;
    out.tangent = vec4<f32>((normal_matrix * local_tangent).xyz, tangent.w);
    return out;
}

fn safe_normalize(value: vec3<f32>) -> vec3<f32> {
    let length_sq = dot(value, value);
    return select(vec3<f32>(0.0, 1.0, 0.0), value * inverseSqrt(length_sq), length_sq > 1e-12);
}

// Trowbridge-Reitz（GGX）法线分布。`alpha` = roughness²。
fn distribution_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-7);
}

// Smith height-correlated visibility（已并入 1/(4 NdotL NdotV)）。
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

// 绕 Y 轴旋转（标准右手系）。环境绕 +Y 转 θ 等价于采样时把方向转 -θ。
// 漫反射与镜面共用。
fn rotate_y(direction: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3<f32>(c * direction.x + s * direction.z, direction.y, -s * direction.x + c * direction.z);
}

//#if IBL_DIFFUSE
// SH9 辐照度。基函数与 `rustlua_assets::env_asset::sh_irradiance` 同一套常数，
// 系数已在烘焙期乘过 cosine 卷积因子，因此返回值已经是 E/π。
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
// Karis 的 EnvBRDFApprox（移动端近似）：给定 F0/roughness/N·V 直接得到
// 预过滤环境的 BRDF 权重，不依赖 LUT。与 `host::renderer::pbr::env_brdf_approx` 同式。
fn env_brdf_approx(f0: vec3<f32>, roughness: f32, n_dot_v: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * n_dot_v)) * r.x + r.y;
    let env = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return max(f0 * env.x + env.y, vec3<f32>(0.0));
}

// 反射方向采样预过滤 cubemap：lod = roughness * (mip_count - 1)。
// 环境旋转同样通过把反射方向转 -θ 实现。
fn sample_environment_specular(normal: vec3<f32>, view: vec3<f32>, roughness: f32) -> vec3<f32> {
    let reflection = rotate_y(reflect(-view, normal), -environment.params.y);
    let lod = clamp(roughness, 0.0, 1.0) * environment.params.w;
    return textureSampleLevel(environment_cube, environment_sampler, reflection, lod).rgb;
}
//#endif

@fragment fn fs_main(@location(0) normal: vec3<f32>, @location(1) shadow_position: vec4<f32>, @location(2) uv: vec2<f32>, @location(3) world_position: vec3<f32>, @location(4) tangent: vec4<f32>, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    let albedo_color = textureSample(albedo, albedo_sampler, uv);
    let base_rgb = material.base_color.rgb * albedo_color.rgb;
    var base_alpha = material.base_color.a * albedo_color.a;
    // P3-R02-4：flags.z = 0/1/2（opaque/mask/blend），flags.w = cutoff。
    // opaque 强制 alpha=1；mask 低于阈值 discard；blend 保留 alpha 交给管线混合。
    if (material.flags.z < 0.5) {
        base_alpha = 1.0;
    }
    if (material.flags.z > 0.5 && material.flags.z < 1.5 && base_alpha < material.flags.w) {
        discard;
    }
    var geometric_normal = safe_normalize(normal);
    // 双面材质没有背面剔除，背面的几何法线要翻转才能得到正确的光照方向。
    if (!front_facing) {
        geometric_normal = -geometric_normal;
    }
    var n = geometric_normal;
    // 法线贴图只在有贴图且切线基可用（w != 0）时启用；UV 退化/镜像时
    // CPU 侧把 w 置 0，这里自然退回几何法线，不再硬算错误切线。
    if (material.params.z > 0.5 && tangent.w != 0.0 && dot(tangent.xyz, tangent.xyz) > 1e-12) {
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

    let shadow_ndc = shadow_position.xyz / shadow_position.w;
    let shadow_uv = vec2<f32>(shadow_ndc.x * 0.5 + 0.5, 1.0 - (shadow_ndc.y * 0.5 + 0.5));
    let shadow_depth = shadow_ndc.z - 0.002;
    let shadow_visibility = select(1.0, textureSampleCompare(shadow_map, shadow_sampler, shadow_uv, shadow_depth), shadow_uv.x >= 0.0 && shadow_uv.x <= 1.0 && shadow_uv.y >= 0.0 && shadow_uv.y <= 1.0 && shadow_ndc.z >= 0.0 && shadow_ndc.z <= 1.0);

    let radiance = light.color_intensity.rgb * light.color_intensity.w;
    // 环境遮蔽只作用在间接项（当前是常数环境光）；直接光与阴影沿用 glTF 语义。
    var occlusion = 1.0;
    if (material.flags.x > 0.5) {
        let ao = textureSample(occlusion_map, albedo_sampler, uv).r;
        occlusion = mix(1.0, ao, clamp(material.flags.y, 0.0, 1.0));
    }
//#if IBL_DIFFUSE
    // P3-R03-2：SH9 漫反射。系数含 cosine 卷积，直接乘反照率；金属没有漫反射。
    var indirect = base_rgb * sh_irradiance(rotate_y(n, -environment.params.y)) * environment.params.x * (1.0 - metallic) * occlusion;
//#else
    //#if IBL_SPECULAR
    // 只有镜面（显式档）：不保留常数环境漫反射，避免和镜面积分重复计入能量。
    var indirect = vec3<f32>(0.0);
    //#else
    // IBL 关闭：保持 R02-1 的常数环境光，回滚档像素与 R02 一致；与旧路径一样不受阴影影响。
    var indirect = base_rgb * AMBIENT_FACTOR * radiance * occlusion;
    //#endif
//#endif
//#if IBL_SPECULAR
    // P3-R03-3：预过滤镜面 IBL。EnvBRDFApprox 给出 F0/roughness/N·V 的权重，
    // mip 由 roughness 线性映射；遮蔽只作用在间接项。
    indirect = indirect + sample_environment_specular(n, v, roughness) * env_brdf_approx(f0, roughness, n_dot_v) * environment.params.x * occlusion;
//#endif
    let emissive = material.emissive.rgb * textureSample(emissive_map, albedo_sampler, uv).rgb;
    return vec4<f32>(direct * radiance * shadow_visibility + indirect + emissive, base_alpha);
}
