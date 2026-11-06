// SSR 双边 3×3：反射颜色按深度差加权，避免半分辨率反射在轮廓上拖出彩边。
struct Params {
    texel: vec4<f32>,
    inverse_view_projection: mat4x4<f32>,
    prev_view_projection: mat4x4<f32>,
};

@group(0) @binding(0) var ssr_source: texture_2d<f32>;
@group(0) @binding(1) var depth_texture: texture_depth_2d;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var ssr_history: texture_2d<f32>;
@group(0) @binding(4) var ssr_history_sampler: sampler;

fn world_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let homogeneous = params.inverse_view_projection * vec4<f32>(ndc, 1.0);
    return homogeneous.xyz / max(homogeneous.w, 1e-6);
}

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
    let size = textureDimensions(ssr_source);
    let uv = position.xy / vec2<f32>(size);
    let center_depth = depth_at(uv);
    let center = textureLoad(ssr_source, vec2<i32>(position.xy), 0);
    let offsets = array<vec2<f32>, 8>(
        vec2<f32>(1.0, 0.0), vec2<f32>(-1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, -1.0),
    );
    var sum = center.rgb * 2.0;
    var hit = center.a * 2.0;
    var weight_sum = 2.0;
    for (var i: i32 = 0; i < 8; i = i + 1) {
        let sample_uv = clamp(
            uv + offsets[i] * params.texel.xy,
            vec2<f32>(0.0),
            vec2<f32>(1.0),
        );
        let sample_depth = depth_at(sample_uv);
        let weight = exp(-abs(sample_depth - center_depth) * 240.0)
            * select(0.75, 1.0, i >= 4);
        let last = vec2<i32>(size) - vec2<i32>(1, 1);
        let coord = clamp(
            vec2<i32>(position.xy) + vec2<i32>(offsets[i]),
            vec2<i32>(0, 0),
            last,
        );
        let sample = textureLoad(ssr_source, coord, 0);
        sum = sum + sample.rgb * weight;
        hit = hit + sample.a * weight;
        weight_sum = weight_sum + weight;
    }
    let scaled = 1.0 / max(weight_sum, 1e-4);
    let current = vec4<f32>(sum * scaled, clamp(hit * scaled, 0.0, 1.0));
    if params.texel.w < 0.5 {
        return current;
    }
    let world = world_from_depth(uv, center_depth);
    let previous = params.prev_view_projection * vec4<f32>(world, 1.0);
    if previous.w <= 1e-4 {
        return current;
    }
    let previous_ndc = previous.xyz / previous.w;
    let previous_uv = vec2<f32>(previous_ndc.x * 0.5 + 0.5, 0.5 - previous_ndc.y * 0.5);
    if previous_uv.x < 0.0 || previous_uv.x > 1.0 || previous_uv.y < 0.0 || previous_uv.y > 1.0 {
        return current;
    }
    // 简单的历史深度拒绝：上一帧屏幕位置在当前深度图上的深度若已跳变，
    // 说明发生了遮挡/去遮挡，历史反射不可信。
    let previous_scene_depth = depth_at(previous_uv);
    if abs(previous_scene_depth - center_depth) > 0.01 {
        return current;
    }
    let history = textureSampleLevel(ssr_history, ssr_history_sampler, previous_uv, 0.0);
    return mix(current, history, 0.75);
}
