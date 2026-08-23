//! WGSL shader sources embedded at compile time (renderer-only).

pub const TEXTURED_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) sdf: u32,
};
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@vertex fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>, @location(3) sdf: u32) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    out.color = color;
    out.sdf = sdf;
    return out;
}
@fragment fn fs_main(@location(0) uv: vec2<f32>, @location(1) color: vec4<f32>, @location(2) sdf: u32) -> @location(0) vec4<f32> {
    let sampled = textureSample(tex, samp, uv);
    if sdf == 1u {
        // 有向距离场：R 通道 = 归一化距离（边缘 0.5），smoothstep 保持锐利。
        // d + 0.08 偏置让笔画更粗（细字重下划线不过细）。
        let d = sampled.r + 0.08;
        let w = max(fwidth(d), 0.01) * 0.5;
        let a = smoothstep(0.5 - w, 0.5 + w, d);
        // 直通 alpha：管线 ALPHA_BLENDING 已按 alpha 预乘源色，不能再乘 a。
        return vec4<f32>(color.rgb, a);
    }
    return sampled * color;
}
"#;

pub const MESH3D_SHADER: &str = r#"
struct Camera { view_projection: mat4x4<f32> };
struct Light { direction: vec4<f32>, color_intensity: vec4<f32> };
struct Material { base_color: vec4<f32>, surface: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> light: Light;
@group(0) @binding(2) var<uniform> shadow_camera: Camera;
@group(0) @binding(3) var shadow_map: texture_depth_2d;
@group(0) @binding(4) var shadow_sampler: sampler_comparison;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var<uniform> joint_palette: array<mat4x4<f32>, 128>;
@group(1) @binding(2) var<storage, read> instance_models: array<mat4x4<f32>>;
@group(1) @binding(3) var albedo: texture_2d<f32>;
@group(1) @binding(4) var albedo_sampler: sampler;
struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) normal: vec3<f32>, @location(1) shadow_position: vec4<f32>, @location(2) uv: vec2<f32> };
@vertex fn vs_main(@builtin(instance_index) instance_index: u32, @location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) joint_indices: vec4<u32>, @location(4) bone_weights: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
    let model = instance_models[instance_index];
    let skinned = material.surface.z > 0.5;
    var local_position = vec4<f32>(position, 1.0);
    var local_normal = vec4<f32>(normal, 0.0);
    if skinned {
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
    }
    let world_position = model * local_position;
    out.position = camera.view_projection * world_position;
    out.normal = local_normal.xyz;
    out.shadow_position = shadow_camera.view_projection * world_position;
    out.uv = uv;
    return out;
}
@fragment fn fs_main(@location(0) normal: vec3<f32>, @location(1) shadow_position: vec4<f32>, @location(2) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let albedo_color = textureSample(albedo, albedo_sampler, uv);
    let base_rgb = material.base_color.rgb * albedo_color.rgb;
    let base_alpha = material.base_color.a * albedo_color.a;
    let n = normalize(normal);
    let l = normalize(-light.direction.xyz);
    let diffuse = max(dot(n, l), 0.0);
    let roughness = clamp(material.surface.y, 0.04, 1.0);
    let metallic = clamp(material.surface.x, 0.0, 1.0);
    let specular = pow(max(dot(reflect(-l, n), vec3<f32>(0.0, 0.0, 1.0)), 0.0), 2.0 / (roughness * roughness));
    let shadow_ndc = shadow_position.xyz / shadow_position.w;
    let shadow_uv = vec2<f32>(shadow_ndc.x * 0.5 + 0.5, 1.0 - (shadow_ndc.y * 0.5 + 0.5));
    let shadow_depth = shadow_ndc.z - 0.002;
    let shadow_visibility = select(1.0, textureSampleCompare(shadow_map, shadow_sampler, shadow_uv, shadow_depth), shadow_uv.x >= 0.0 && shadow_uv.x <= 1.0 && shadow_uv.y >= 0.0 && shadow_uv.y <= 1.0 && shadow_ndc.z >= 0.0 && shadow_ndc.z <= 1.0);
    let brightness = (0.15 + (diffuse * (1.0 - metallic) + specular * mix(0.04, 1.0, metallic)) * shadow_visibility) * light.color_intensity.w;
    return vec4<f32>(base_rgb * light.color_intensity.rgb * brightness, base_alpha);
}
"#;

pub const SHADOW_SHADER: &str = r#"
struct Camera { view_projection: mat4x4<f32> };
struct Material { base_color: vec4<f32>, surface: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var<uniform> joint_palette: array<mat4x4<f32>, 128>;
@group(1) @binding(2) var<storage, read> instance_models: array<mat4x4<f32>>;
@vertex fn vs_main(@builtin(instance_index) instance_index: u32, @location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) joint_indices: vec4<u32>, @location(4) bone_weights: vec4<f32>) -> @builtin(position) vec4<f32> {
    let model = instance_models[instance_index];
    let skinned = material.surface.z > 0.5;
    var local_position = vec4<f32>(position, 1.0);
    if skinned {
        var skinned_position = vec3<f32>(0.0);
        skinned_position += (joint_palette[joint_indices.x] * local_position).xyz * bone_weights.x;
        skinned_position += (joint_palette[joint_indices.y] * local_position).xyz * bone_weights.y;
        skinned_position += (joint_palette[joint_indices.z] * local_position).xyz * bone_weights.z;
        skinned_position += (joint_palette[joint_indices.w] * local_position).xyz * bone_weights.w;
        local_position = vec4<f32>(skinned_position, 1.0);
    }
    return camera.view_projection * model * local_position;
}
"#;
