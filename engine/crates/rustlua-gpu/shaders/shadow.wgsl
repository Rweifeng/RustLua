struct Camera { view_projection: mat4x4<f32> };
struct Material {
    base_color: vec4<f32>,
    surface: vec4<f32>,
    emissive: vec4<f32>,
    params: vec4<f32>,
    flags: vec4<f32>,
};
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var<uniform> joint_palette: array<mat4x4<f32>, 128>;
//#if STORAGE_INSTANCES
@group(1) @binding(2) var<storage, read> instance_models: array<mat4x4<f32>>;
//#else
// WebGL2 没有存储缓冲：每条 draw 一个模型矩阵，走 64 字节 uniform（实例化合批已关）。
@group(1) @binding(2) var<uniform> instance_model: mat4x4<f32>;
//#endif
// P3-R02-4：mask 材质的阴影按 base color 贴图 alpha 做阈值剔除。
// opaque 分支不采样（flags.z == 0），因此旧场景的深度结果逐字节不变。
@group(1) @binding(3) var albedo: texture_2d<f32>;
@group(1) @binding(4) var albedo_sampler: sampler;
@vertex fn vs_main(@builtin(instance_index) instance_index: u32, @location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) joint_indices: vec4<u32>, @location(4) bone_weights: vec4<f32>) -> VertexOutput {
//#if STORAGE_INSTANCES
    let model = instance_models[instance_index];
//#else
    let model = instance_model;
//#endif
    var local_position = vec4<f32>(position, 1.0);
//#if SKIN
    var skinned_position = vec3<f32>(0.0);
    skinned_position += (joint_palette[joint_indices.x] * local_position).xyz * bone_weights.x;
    skinned_position += (joint_palette[joint_indices.y] * local_position).xyz * bone_weights.y;
    skinned_position += (joint_palette[joint_indices.z] * local_position).xyz * bone_weights.z;
    skinned_position += (joint_palette[joint_indices.w] * local_position).xyz * bone_weights.w;
    local_position = vec4<f32>(skinned_position, 1.0);
//#endif
    var out: VertexOutput;
    out.position = camera.view_projection * model * local_position;
    out.uv = uv;
    return out;
}

@fragment fn fs_main(in: VertexOutput) {
    if (material.flags.z > 0.5 && material.flags.z < 1.5) {
        let alpha = material.base_color.a * textureSample(albedo, albedo_sampler, in.uv).a;
        if (alpha < material.flags.w) {
            discard;
        }
    }
}
