// 半分辨率 SSR-lite：从深度重建世界坐标与法线，沿反射方向做 10 步定长推进，
// 命中后取 HDR 场景颜色并按菲涅耳衰减。输出 Rgba16Float（rgb=反射，a=命中）。
struct Params {
    inverse_view_projection: mat4x4<f32>,
    view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
};

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var depth_texture: texture_depth_2d;
@group(0) @binding(3) var<uniform> params: Params;

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

fn world_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let homogeneous = params.inverse_view_projection * vec4<f32>(ndc, 1.0);
    return homogeneous.xyz / max(homogeneous.w, 1e-6);
}

fn world_normal(uv: vec2<f32>, world: vec3<f32>) -> vec3<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(depth_texture));
    let right_uv = clamp(uv + vec2<f32>(texel.x, 0.0), vec2<f32>(0.0), vec2<f32>(1.0));
    let up_uv = clamp(uv + vec2<f32>(0.0, texel.y), vec2<f32>(0.0), vec2<f32>(1.0));
    let right = world_from_depth(right_uv, depth_at(right_uv));
    let up = world_from_depth(up_uv, depth_at(up_uv));
    return normalize(cross(right - world, up - world));
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
    let dimensions = vec2<f32>(textureDimensions(depth_texture));
    let uv = position.xy / dimensions;
    let center_depth = depth_at(uv);
    if center_depth >= 0.9999 {
        return vec4<f32>(0.0);
    }
    let world = world_from_depth(uv, center_depth);
    var normal = world_normal(uv, world);
    let view = normalize(params.camera_position.xyz - world);
    if dot(normal, view) < 0.0 {
        normal = -normal;
    }
    let reflected = reflect(-view, normal);
    let origin = world + normal * 0.02;
    var done = false;
    for (var step: i32 = 1; step <= 10; step = step + 1) {
        if (!done) {
            let travel = 0.18 * f32(step) * f32(step);
            let sample = origin + reflected * travel;
            let clip = params.view_projection * vec4<f32>(sample, 1.0);
            let ndc = clip.xyz / max(clip.w, 1e-4);
            let sample_uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
            let outside = clip.w <= 1e-4
                || sample_uv.x < 0.0 || sample_uv.x > 1.0
                || sample_uv.y < 0.0 || sample_uv.y > 1.0;
            if (outside) {
                done = true;
            } else {
                let scene_depth = depth_at(sample_uv);
                let delta = ndc.z - scene_depth;
                if (scene_depth < 0.9999 && delta > 0.0015 && delta < 0.05) {
                    let reflected_color = textureSampleLevel(source, source_sampler, sample_uv, 0.0).rgb;
                    let fresnel = pow(1.0 - clamp(dot(normal, view), 0.0, 1.0), 3.0);
                    return vec4<f32>(reflected_color * (0.12 + 0.88 * fresnel), 1.0);
                }
            }
        }
    }
    return vec4<f32>(0.0);
}
