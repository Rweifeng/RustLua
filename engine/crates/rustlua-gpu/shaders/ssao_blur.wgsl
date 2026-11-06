// SSAO 双边 3×3 模糊：以深度差为权重，边缘两侧的 AO 不互相拖带。
struct Params {
    texel: vec4<f32>,
};

@group(0) @binding(0) var ao_source: texture_2d<f32>;
@group(0) @binding(1) var depth_texture: texture_depth_2d;
@group(0) @binding(2) var<uniform> params: Params;

fn depth_at(uv: vec2<f32>) -> f32 {
    let dimensions = vec2<i32>(textureDimensions(depth_texture));
    let last = max(dimensions - vec2<i32>(1, 1), vec2<i32>(0, 0));
    let coord = clamp(
        vec2<i32>(clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)) * vec2<f32>(dimensions)),
        vec2<i32>(0, 0),
        last,
    );
    return textureLoad(depth_texture, coord, 0);
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[index], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let ao_size = textureDimensions(ao_source);
    let uv = position.xy / vec2<f32>(ao_size);
    let center_depth = depth_at(uv);
    let center_ao = textureLoad(ao_source, vec2<i32>(position.xy), 0).r;
    let offsets = array<vec2<f32>, 8>(
        vec2<f32>(1.0, 0.0), vec2<f32>(-1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, -1.0),
    );
    var sum = center_ao * 2.0;
    var weight_sum = 2.0;
    for (var i: i32 = 0; i < 8; i = i + 1) {
        let sample_uv = clamp(
            uv + offsets[i] * params.texel.xy,
            vec2<f32>(0.0),
            vec2<f32>(1.0),
        );
        let sample_depth = depth_at(sample_uv);
        let depth_weight = exp(-abs(sample_depth - center_depth) * 240.0);
        let weight = depth_weight * select(0.75, 1.0, i >= 4);
        let ao_last = vec2<i32>(ao_size) - vec2<i32>(1, 1);
        let ao_coord = clamp(
            vec2<i32>(position.xy) + vec2<i32>(offsets[i]),
            vec2<i32>(0, 0),
            ao_last,
        );
        let sample_ao = textureLoad(ao_source, ao_coord, 0).r;
        sum = sum + sample_ao * weight;
        weight_sum = weight_sum + weight;
    }
    let ao = sum / max(weight_sum, 1e-4);
    return vec4<f32>(ao, ao, ao, 1.0);
}
