// 半分辨率 SSAO：从全分辨率深度重建世界坐标与法线，做 3 圈 × 4 方向半球采样，
// 再加 8 方向深度断层回退。输出 R8Unorm AO，由 Tonemap 双线性升采样 + 5 抽头。
struct Params {
    inverse_view_projection: mat4x4<f32>,
    view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
};

@group(0) @binding(0) var depth_texture: texture_depth_2d;
@group(0) @binding(1) var<uniform> params: Params;

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

fn hash12(p: vec2<f32>) -> f32 {
    let h = dot(p, vec2<f32>(127.1, 311.7));
    return fract(sin(h) * 43758.5453);
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
    let depth = depth_at(uv);
    if depth >= 0.9999 {
        return vec4<f32>(1.0, 1.0, 1.0, 1.0);
    }
    let world = world_from_depth(uv, depth);
    var normal = world_normal(uv, world);
    let to_camera = normalize(params.camera_position.xyz - world);
    if dot(normal, to_camera) < 0.0 {
        normal = -normal;
    }
    let reference = select(
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(1.0, 0.0, 0.0),
        abs(normal.y) > 0.9,
    );
    let tangent = normalize(cross(reference, normal));
    let bitangent = cross(normal, tangent);
    let angle_offset = hash12(uv * vec2<f32>(1920.0, 1080.0)) * 6.2831853;
    var occlusion = 0.0;
    for (var i: i32 = 0; i < 12; i = i + 1) {
        let ring = f32(i / 4);
        let angle = angle_offset + f32(i % 4) * 1.5707963;
        let lift = 0.25 + ring * 0.25;
        let dir = normalize(tangent * cos(angle) + bitangent * sin(angle) + normal * lift);
        let sample = world + dir * (0.18 + ring * 0.12);
        let clip = params.view_projection * vec4<f32>(sample, 1.0);
        if clip.w > 1e-4 {
            let ndc = clip.xyz / clip.w;
            let sample_uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
            let inside = sample_uv.x >= 0.0 && sample_uv.x <= 1.0
                && sample_uv.y >= 0.0 && sample_uv.y <= 1.0;
            if inside {
                let scene_depth = depth_at(sample_uv);
                if scene_depth < ndc.z - 0.0015 {
                    occlusion = occlusion + 1.0;
                }
            }
        }
    }
    let radius = 36.0 / dimensions.y;
    let edge_dirs = array<vec2<f32>, 8>(
        vec2<f32>(1.0, 0.0), vec2<f32>(-1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(0.0, -1.0),
        vec2<f32>(0.707, 0.707), vec2<f32>(-0.707, 0.707),
        vec2<f32>(0.707, -0.707), vec2<f32>(-0.707, -0.707),
    );
    var edge = 0.0;
    for (var j: i32 = 0; j < 8; j = j + 1) {
        let sample_uv = uv + edge_dirs[j] * radius;
        if sample_uv.x >= 0.0 && sample_uv.x <= 1.0
            && sample_uv.y >= 0.0 && sample_uv.y <= 1.0 {
            let sample_depth = depth_at(sample_uv);
            if sample_depth > 0.0 && sample_depth < depth - 0.003 {
                edge = edge + 1.0;
            }
        }
    }
    occlusion = occlusion + edge;
    let ao = clamp(1.0 - occlusion * 0.075, 0.52, 1.0);
    return vec4<f32>(ao, ao, ao, 1.0);
}
