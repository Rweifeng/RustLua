struct Params {
    exposure_ev: f32,
    tonemap: f32,
    _pad0: f32,
    _pad1: f32,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex fn vs_main(@builtin(vertex_index) index: u32) -> Out {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let corner = positions[index];
    var out: Out;
    out.position = vec4<f32>(corner, 0.0, 1.0);
    out.uv = vec2<f32>((corner.x + 1.0) * 0.5, (1.0 - corner.y) * 0.5);
    return out;
}

fn aces(x: f32) -> f32 {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}

fn reinhard(x: f32) -> f32 {
    return clamp(x / (1.0 + x), 0.0, 1.0);
}

@fragment fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let sampled = textureSample(source, source_sampler, uv);
    let gain = exp2(params.exposure_ev);
    let linear = max(sampled.rgb * gain, vec3<f32>(0.0));
    let mapped = vec3<f32>(
        select(aces(linear.r), reinhard(linear.r), params.tonemap > 0.5),
        select(aces(linear.g), reinhard(linear.g), params.tonemap > 0.5),
        select(aces(linear.b), reinhard(linear.b), params.tonemap > 0.5),
    );
    return vec4<f32>(mapped, clamp(sampled.a, 0.0, 1.0));
}
