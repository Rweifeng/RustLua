struct Params {
    exposure: f32,
    vignette: f32,
    exposure_ev: f32,
    _pad0: f32,
    tint: vec4<f32>,
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

@fragment fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let sampled = textureSample(source, source_sampler, uv);
    let gain = params.exposure * exp2(params.exposure_ev);
    let tinted = sampled.rgb * gain * params.tint.rgb;
    let centered = uv - vec2<f32>(0.5, 0.5);
    let falloff = 1.0 - params.vignette * clamp(dot(centered, centered) * 4.0, 0.0, 1.0);
    return vec4<f32>(tinted * falloff, sampled.a * params.tint.a);
}
