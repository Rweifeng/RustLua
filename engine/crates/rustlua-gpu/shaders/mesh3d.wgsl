struct Camera { view_projection: mat4x4<f32> };
struct ShadowCameras { view_projection: array<mat4x4<f32>, 4> };
struct PointShadow {
    cameras: array<mat4x4<f32>, 48>,
    params: array<vec4<f32>, 8>,
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
struct Material { base_color: vec4<f32>, surface: vec4<f32> };
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
struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) normal: vec3<f32>, @location(1) shadow_position: vec4<f32>, @location(2) uv: vec2<f32>, @location(3) world_position: vec3<f32>, @location(4) shadow_position_mid: vec4<f32>, @location(5) shadow_position_far: vec4<f32>, @location(6) shadow_position_ultra: vec4<f32> };
@vertex fn vs_main(@builtin(instance_index) instance_index: u32, @location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) joint_indices: vec4<u32>, @location(4) bone_weights: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
    var local_position = vec4<f32>(position, 1.0);
    var local_normal = vec4<f32>(normal, 0.0);
//#if STORAGE_INSTANCES
    let model = instance_models[instance_index];
//#else
    let model = instance_model;
//#endif
//#if SKIN
    var skinned_position = vec3<f32>(0.0);
    var skinned_normal = vec3<f32>(0.0);
    skinned_position += (joint_palette[joint_indices.x] * local_position).xyz * bone_weights.x;
    skinned_position += (joint_palette[joint_indices.y] * local_position).xyz * bone_weights.y;
    skinned_position += (joint_palette[joint_indices.z] * local_position).xyz * bone_weights.z;
    skinned_position += (joint_palette[joint_indices.w] * local_position).xyz * bone_weights.w;
    skinned_normal += (joint_palette[joint_indices.x] * local_normal).xyz * bone_weights.x;
    skinned_normal += (joint_palette[joint_indices.y] * local_normal).xyz * bone_weights.y;
    skinned_normal += (joint_palette[joint_indices.z] * local_normal).xyz * bone_weights.z;
    skinned_normal += (joint_palette[joint_indices.w] * local_normal).xyz * bone_weights.w;
    local_position = vec4<f32>(skinned_position, 1.0);
    local_normal = vec4<f32>(skinned_normal, 0.0);
//#endif
    let world_position = model * local_position;
    out.position = camera.view_projection * world_position;
    out.normal = local_normal.xyz;
    out.shadow_position = shadow_camera.view_projection[0] * world_position;
    out.shadow_position_mid = shadow_camera.view_projection[1] * world_position;
    out.shadow_position_far = shadow_camera.view_projection[2] * world_position;
    out.shadow_position_ultra = shadow_camera.view_projection[3] * world_position;
    out.uv = uv;
    out.world_position = world_position.xyz;
    return out;
}
// 方向光阴影：默认单次硬件比较；`light.params.y` 打开后做 PCSS-lite
// 5×5 遮挡体搜 + 按遮挡距离放大的 5×5 PCF，做出联系硬化（贴地硬、离地软）
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

fn cascade_coords(clip: vec4<f32>) -> vec4<f32> {
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 1.0 - (ndc.y * 0.5 + 0.5));
    let inside = ndc.z >= 0.0 && ndc.z <= 1.0 &&
        uv.x >= 0.0 && uv.x <= 1.0 &&
        uv.y >= 0.0 && uv.y <= 1.0;
    return vec4<f32>(uv, ndc.z, select(0.0, 1.0, inside));
}

fn cascade_edge(uv: vec2<f32>) -> f32 {
    return max(abs(uv.x * 2.0 - 1.0), abs(uv.y * 2.0 - 1.0));
}

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

fn point_shadow_slot(light_position: vec3<f32>) -> i32 {
    for (var slot = 0u; slot < 8u; slot = slot + 1u) {
        let params = point_shadow.params[slot];
        if (params.w > 0.5 && distance(light_position, params.xyz) < 1e-3) {
            return i32(slot);
        }
    }
    return -1;
}

fn point_shadow_visibility(world_position: vec3<f32>, slot: u32) -> f32 {
    let params = point_shadow.params[slot];
    let light_position = params.xyz;
    let delta = world_position - light_position;
    let distance_to_light = length(delta);
    if (distance_to_light < 1e-3 || distance_to_light > params.w) {
        return 1.0;
    }
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

@fragment fn fs_main(@location(0) normal: vec3<f32>, @location(1) shadow_position: vec4<f32>, @location(2) uv: vec2<f32>, @location(3) world_position: vec3<f32>, @location(4) shadow_position_mid: vec4<f32>, @location(5) shadow_position_far: vec4<f32>, @location(6) shadow_position_ultra: vec4<f32>) -> @location(0) vec4<f32> {
    let albedo_color = textureSample(albedo, albedo_sampler, uv);
    let base_rgb = material.base_color.rgb * albedo_color.rgb;
    let base_alpha = material.base_color.a * albedo_color.a;
    let n = normalize(normal);
    let l = normalize(-light.direction.xyz);
    let diffuse = max(dot(n, l), 0.0);
    let roughness = clamp(material.surface.y, 0.04, 1.0);
    let metallic = clamp(material.surface.x, 0.0, 1.0);
    let specular = pow(max(dot(reflect(-l, n), vec3<f32>(0.0, 0.0, 1.0)), 0.0), 2.0 / (roughness * roughness));
    let shadow_visibility = shadow_visibility_four_cascades(
        shadow_position,
        shadow_position_mid,
        shadow_position_far,
        shadow_position_ultra,
    );
    let brightness = (0.15 + (diffuse * (1.0 - metallic) + specular * mix(0.04, 1.0, metallic)) * shadow_visibility) * light.color_intensity.w;
    // 点光源：同一 Lambert/近似镜面，衰减用"range 内平方反 × 平滑截断"
    // 第一盏在点光阴影启用时按六面深度图可见性压暗
    var point_lit = vec3<f32>(0.0);
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
        if (point_n_dot_l <= 0.0) {
            continue;
        }
        let point_specular = pow(max(dot(reflect(-point_l, n), vec3<f32>(0.0, 0.0, 1.0)), 0.0), 2.0 / (roughness * roughness));
        let point_lambert = point_n_dot_l * (1.0 - metallic) + point_specular * mix(0.04, 1.0, metallic);
        let falloff = clamp(1.0 - distance / max(point.position_range.w, 1e-4), 0.0, 1.0);
        let attenuation = falloff * falloff / (distance * distance + 1e-4);
        var point_visibility = 1.0;
        let shadow_slot = point_shadow_slot(point.position_range.xyz);
        if (shadow_slot >= 0) {
            point_visibility = point_shadow_visibility(world_position, u32(shadow_slot));
        }
        point_lit = point_lit + base_rgb * point.color_intensity.rgb * point.color_intensity.w * point_lambert * attenuation * point_visibility;
    }
    return vec4<f32>(base_rgb * light.color_intensity.rgb * brightness + point_lit, base_alpha);
}
