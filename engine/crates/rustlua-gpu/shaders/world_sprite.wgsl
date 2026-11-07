struct Camera {
    view_projection: mat4x4<f32>,
    // x = 时间（秒）；yzw 预留
    time: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec3<f32>,
};
@vertex fn vs_main(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = camera.view_projection * vec4<f32>(position, 1.0);
    out.uv = uv;
    out.color = color;
    out.world = position;
    return out;
}

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

// P3-R64：世界 Sprite（蚂蚁/草簇/标签）与 3D 网格吃同一片移动云影。
fn cloud_shadow(world: vec2<f32>, time: f32, coverage: f32, speed: f32) -> f32 {
    let drift = vec2<f32>(time * speed * 0.35, time * speed * 0.12);
    let p = world * 0.0035 + drift * 0.05;
    let n = value_noise(p) * 0.62 + value_noise(p * 2.3 + vec2<f32>(7.1, 3.7)) * 0.38;
    let bias = (clamp(coverage, 0.0, 1.0) - 0.5) * 0.18;
    return smoothstep(0.50 - bias, 0.74 - bias, n);
}

@fragment fn fs_main(
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec3<f32>,
) -> @location(0) vec4<f32> {
    let cloud = cloud_shadow(world.xz, camera.time.x, camera.time.y, camera.time.z);
    return textureSample(tex, samp, uv) * color * (1.0 - cloud * camera.time.w);
}
