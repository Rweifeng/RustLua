// 方向光 ESM 的生成 pass：与 CSM 同分辨率（2048²）把深度写成 exp(depth * 10)。
// 主着色器再对这张滤波图做 3×3 平均，用 exp(-10 * receiver_depth) 恢复可见度，
// 得到比单张深度图更软的边缘。
struct Layer {
    index: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var shadow_depth: texture_depth_2d_array;
@group(0) @binding(1) var<uniform> layer: Layer;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let source = vec2<i32>(position.xy);
    let last = vec2<i32>(textureDimensions(shadow_depth)) - vec2<i32>(1, 1);
    let depth = textureLoad(shadow_depth, clamp(source, vec2<i32>(0, 0), last), i32(layer.index), 0);
    return vec4<f32>(exp(depth * 10.0), 0.0, 0.0, 1.0);
}
