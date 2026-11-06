struct Params {
    exposure_ev: f32,
    tonemap: f32,
    bloom: f32,
    fxaa: f32,
    ssao: f32,
    ssr: f32,
    contact_shadows: f32,
    god_rays: f32,
    view_projection: mat4x4<f32>,
    inverse_view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
    light_direction: vec4<f32>,
    // x = 密度（0 = 关），yzw = 雾色
    fog: vec4<f32>,
    // x = 开关，y = 对焦距离，z = 过渡范围，w = 强度
    dof: vec4<f32>,
    sky: vec4<f32>,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var depth_texture: texture_depth_2d;
@group(0) @binding(4) var ao_map: texture_2d<f32>;
@group(0) @binding(5) var ao_sampler: sampler;
@group(0) @binding(6) var ssr_map: texture_2d<f32>;
@group(0) @binding(7) var ssr_sampler: sampler;

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

// 三个半径 × 8 方向：近环保留高光边缘，中/远环给上帝视角的大范围辉光。
// 半径按 900 逻辑高缩放，分辨率变化时辉光占屏比例基本不变。
// 半径以 900 逻辑高为基准按分辨率缩放；只取超过 1.0 的 HDR 能量，避免把普通亮面整体糊掉。
fn bloom_glow(uv: vec2<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(source));
    let texel = 1.0 / size;
    let scale = max(size.y / 900.0, 0.5);
    let radii = array<f32, 3>(2.5 * scale, 8.0 * scale, 18.0 * scale);
    let weights = array<f32, 3>(0.5, 0.3, 0.2);
    let dirs = array<vec2<f32>, 8>(
        vec2<f32>(1.0, 0.0), vec2<f32>(-1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(0.0, -1.0),
        vec2<f32>(0.707, 0.707), vec2<f32>(-0.707, 0.707),
        vec2<f32>(0.707, -0.707), vec2<f32>(-0.707, -0.707),
    );
    var sum = vec3<f32>(0.0);
    for (var ring: i32 = 0; ring < 3; ring = ring + 1) {
        for (var i: i32 = 0; i < 8; i = i + 1) {
            let s = textureSample(source, source_sampler, uv + dirs[i] * radii[ring] * texel);
            sum = sum + max(s.rgb - vec3<f32>(1.0), vec3<f32>(0.0)) * weights[ring];
        }
    }
    return sum * 0.125;
}

fn tonemap_rgb(linear: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        select(aces(linear.r), reinhard(linear.r), params.tonemap > 0.5),
        select(aces(linear.g), reinhard(linear.g), params.tonemap > 0.5),
        select(aces(linear.b), reinhard(linear.b), params.tonemap > 0.5),
    );
}

fn tonemap_at(uv: vec2<f32>) -> vec3<f32> {
    let sampled = textureSample(source, source_sampler, uv);
    return tonemap_rgb(max(sampled.rgb * exp2(params.exposure_ev), vec3<f32>(0.0)));
}

fn luma(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// 屏幕空间神光：把太阳方向投到屏幕，沿“像素 → 太阳”的方向累积亮部。
// 太阳在屏外/相机背后时直接返回 0，不生成屏外几何的补全。
fn god_rays_at(uv: vec2<f32>) -> vec3<f32> {
    if params.god_rays < 0.5 {
        return vec3<f32>(0.0);
    }
    let to_light = params.light_direction.xyz;
    if dot(to_light, to_light) < 1e-8 {
        return vec3<f32>(0.0);
    }
    let sun_world = params.camera_position.xyz + normalize(to_light) * 1000.0;
    let clip = params.view_projection * vec4<f32>(sun_world, 1.0);
    if clip.w <= 1e-4 {
        return vec3<f32>(0.0);
    }
    let ndc = clip.xyz / clip.w;
    let sun_uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if sun_uv.x < -0.5 || sun_uv.x > 1.5 || sun_uv.y < -0.5 || sun_uv.y > 1.5 {
        return vec3<f32>(0.0);
    }
    let delta = (sun_uv - uv) / 12.0;
    var sample_uv = uv;
    var accumulation = 0.0;
    for (var i: i32 = 0; i < 12; i = i + 1) {
        sample_uv = sample_uv + delta;
        let clamped = clamp(sample_uv, vec2<f32>(0.0), vec2<f32>(1.0));
        let sampled = textureSampleLevel(source, source_sampler, clamped, 0.0).rgb;
        accumulation = accumulation + max(luma(sampled) - 0.02, 0.0);
    }
    // 太阳盘/光晕：只在太阳真的落在屏幕内时叠；天空像素画实心盘，前景只留柔光。
    let aspect = vec2<f32>(
        f32(textureDimensions(source).x) / f32(textureDimensions(source).y),
        1.0,
    );
    let sun_distance = length((uv - sun_uv) * aspect);
    let glow = exp(-sun_distance * 16.0) * 0.35;
    var sun = vec3<f32>(1.0, 0.90, 0.72) * glow;
    if (depth_at(uv) >= 0.9999) {
        sun = sun + vec3<f32>(1.0, 0.86, 0.58) * smoothstep(0.045, 0.012, sun_distance) * 1.6;
    }
    return vec3<f32>(1.0, 0.90, 0.72) * (accumulation / 12.0) * 0.5 + sun;
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

fn world_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let homogeneous = params.inverse_view_projection * vec4<f32>(ndc, 1.0);
    return homogeneous.xyz / max(homogeneous.w, 1e-6);
}

// 程序化天空：只在 depth=1 的背景像素替换清屏色；相机射线由逆视图投影重建。
// 地平线（暖灰蓝）→ 天顶（深蓝）的渐变 + 太阳盘 + 柔和日晕；与距离雾同一开关。
fn sky_color(uv: vec2<f32>) -> vec3<f32> {
    let far = world_from_depth(uv, 1.0);
    let ray = normalize(far - params.camera_position.xyz);
    let height = clamp(ray.y, 0.0, 1.0);
    let zenith = vec3<f32>(0.055, 0.14, 0.34);
    let horizon = vec3<f32>(0.55, 0.68, 0.80);
    var color = mix(horizon, zenith, pow(height, max(params.sky.z, 0.05)));
    // 地平线以下给一点空气透视，避免天空与地面之间出现硬边。
    color = mix(vec3<f32>(0.34, 0.38, 0.41), color, smoothstep(-0.18, 0.01, ray.y));
    let light_len = max(length(params.light_direction.xyz), 1e-6);
    let to_sun = params.light_direction.xyz / light_len;
    let sun = max(dot(ray, to_sun), 0.0);
    color = color
        + vec3<f32>(1.0, 0.82, 0.58)
            * (pow(sun, 1400.0) * 6.0 + pow(sun, 24.0) * 0.10)
            * params.sky.y;
    return color;
}

// 距离雾/空气透视：按相机到重建世界点的距离做指数衰减，天空像素不参与。
fn fog_at(uv: vec2<f32>) -> f32 {
    if params.fog.x <= 0.0 {
        return 0.0;
    }
    let depth = depth_at(uv);
    if depth >= 0.9999 {
        return 0.0;
    }
    let world = world_from_depth(uv, depth);
    let distance = length(params.camera_position.xyz - world);
    return clamp(1.0 - exp(-params.fog.x * distance), 0.0, 0.92);
}

// 距离景深：用重建世界点到相机的距离算 CoC，8 方向模糊；对焦点附近不动。
fn depth_of_field(uv: vec2<f32>, center: vec3<f32>) -> vec3<f32> {
    if params.dof.x < 0.5 {
        return center;
    }
    let depth = depth_at(uv);
    if depth >= 0.9999 {
        return center;
    }
    let world = world_from_depth(uv, depth);
    let distance = length(params.camera_position.xyz - world);
    let coc = clamp(
        (abs(distance - params.dof.y) / max(params.dof.z, 0.1)) * params.dof.w,
        0.0,
        1.0,
    );
    if coc <= 0.01 {
        return center;
    }
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    let radius = coc * 10.0;
    let dirs = array<vec2<f32>, 8>(
        vec2<f32>(1.0, 0.0), vec2<f32>(-1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(0.0, -1.0),
        vec2<f32>(0.707, 0.707), vec2<f32>(-0.707, 0.707),
        vec2<f32>(0.707, -0.707), vec2<f32>(-0.707, -0.707),
    );
    var sum = vec3<f32>(0.0);
    for (var i: i32 = 0; i < 8; i = i + 1) {
        sum = sum + textureSampleLevel(source, source_sampler, uv + dirs[i] * texel * radius, 0.0).rgb;
    }
    return mix(center, sum / 8.0, coc);
}

fn world_normal(uv: vec2<f32>, world: vec3<f32>) -> vec3<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(depth_texture));
    let right_uv = clamp(uv + vec2<f32>(texel.x, 0.0), vec2<f32>(0.0), vec2<f32>(1.0));
    let up_uv = clamp(uv + vec2<f32>(0.0, texel.y), vec2<f32>(0.0), vec2<f32>(1.0));
    let right = world_from_depth(right_uv, depth_at(right_uv));
    let up = world_from_depth(up_uv, depth_at(up_uv));
    return normalize(cross(right - world, up - world));
}

// 接触阴影：沿“指向光”的方向在屏幕空间推进 8 步，若某步的深度被更近的几何挡住，
// 就压暗当前像素。它补 CSM 分辨率不足时物体根部/缝隙里丢掉的小尺度接触暗部。
fn contact_shadow_at(uv: vec2<f32>) -> f32 {
    if params.contact_shadows < 0.5 {
        return 1.0;
    }
    let center_depth = depth_at(uv);
    if center_depth >= 0.9999 {
        return 1.0;
    }
    let world = world_from_depth(uv, center_depth);
    var normal = world_normal(uv, world);
    let to_light = params.light_direction.xyz;
    if dot(to_light, to_light) < 1e-8 {
        return 1.0;
    }
    let light_dir = normalize(to_light);
    if dot(normal, light_dir) <= 0.05 {
        return 1.0;
    }
    let origin = world + normal * 0.02;
    var occlusion = 0.0;
    var done = false;
    for (var step: i32 = 1; step <= 8; step = step + 1) {
        if (!done) {
            let sample = origin + light_dir * (0.04 * f32(step));
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
                if (scene_depth < ndc.z - 0.001) {
                    occlusion = occlusion + 1.0;
                }
            }
        }
    }
    return clamp(1.0 - occlusion * 0.12, 0.55, 1.0);
}

@fragment fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let sampled = textureSample(source, source_sampler, uv);
    let gain = exp2(params.exposure_ev);
    var linear = max(sampled.rgb * gain, vec3<f32>(0.0));
    if params.bloom > 0.0 {
        linear = linear + bloom_glow(uv) * params.bloom * 2.0;
    }
    linear = depth_of_field(uv, linear);
    if params.ssao > 0.5 {
        // 半分辨率 AO 双线性升采样 + 5 抽头软化，避免半分辨率块状感。
        let ao_texel = 1.0 / vec2<f32>(textureDimensions(ao_map));
        var ao = textureSampleLevel(ao_map, ao_sampler, uv, 0.0).r * 0.40;
        ao = ao + textureSampleLevel(ao_map, ao_sampler, uv + vec2<f32>(ao_texel.x, 0.0), 0.0).r * 0.15;
        ao = ao + textureSampleLevel(ao_map, ao_sampler, uv - vec2<f32>(ao_texel.x, 0.0), 0.0).r * 0.15;
        ao = ao + textureSampleLevel(ao_map, ao_sampler, uv + vec2<f32>(0.0, ao_texel.y), 0.0).r * 0.15;
        ao = ao + textureSampleLevel(ao_map, ao_sampler, uv - vec2<f32>(0.0, ao_texel.y), 0.0).r * 0.15;
        linear = linear * ao;
    }
    if params.contact_shadows > 0.5 {
        linear = linear * contact_shadow_at(uv);
    }
    if params.ssr > 0.5 {
        let ssr_texel = 1.0 / vec2<f32>(textureDimensions(ssr_map));
        var reflected = textureSampleLevel(ssr_map, ssr_sampler, uv, 0.0).rgb * 0.40;
        reflected = reflected + textureSampleLevel(ssr_map, ssr_sampler, uv + vec2<f32>(ssr_texel.x, 0.0), 0.0).rgb * 0.15;
        reflected = reflected + textureSampleLevel(ssr_map, ssr_sampler, uv - vec2<f32>(ssr_texel.x, 0.0), 0.0).rgb * 0.15;
        reflected = reflected + textureSampleLevel(ssr_map, ssr_sampler, uv + vec2<f32>(0.0, ssr_texel.y), 0.0).rgb * 0.15;
        reflected = reflected + textureSampleLevel(ssr_map, ssr_sampler, uv - vec2<f32>(0.0, ssr_texel.y), 0.0).rgb * 0.15;
        linear = linear + reflected * 0.45;
    }
    if params.sky.x > 0.5 && depth_at(uv) >= 0.9999 {
        linear = sky_color(uv);
    }
    linear = linear + god_rays_at(uv);
    linear = mix(linear, params.fog.yzw, fog_at(uv));
    var mapped = tonemap_rgb(linear);
    // FXAA-lite：5 采样、按亮度对比选边方向；只在 3D 的 Tonemap 阶段执行，
    // UI 在这之后绘制，因此文字/描边不会被糊。
    if params.fxaa > 0.5 {
        let texel = 1.0 / vec2<f32>(textureDimensions(source));
        let left = tonemap_at(uv - vec2<f32>(texel.x, 0.0));
        let right = tonemap_at(uv + vec2<f32>(texel.x, 0.0));
        let up = tonemap_at(uv + vec2<f32>(0.0, -texel.y));
        let down = tonemap_at(uv + vec2<f32>(0.0, texel.y));
        let center_luma = luma(mapped);
        let min_luma = min(min(luma(left), luma(right)), min(luma(up), luma(down)));
        let max_luma = max(max(luma(left), luma(right)), max(luma(up), luma(down)));
        let contrast = max(max_luma, center_luma) - min(min_luma, center_luma);
        if contrast > 0.08 {
            let horizontal = abs(luma(left) - luma(right));
            let vertical = abs(luma(up) - luma(down));
            let along = select((left + right) * 0.5, (up + down) * 0.5, vertical < horizontal);
            let weight = clamp((contrast - 0.08) * 2.0, 0.0, 0.5);
            mapped = mix(mapped, along, weight);
        }
    }
    return vec4<f32>(mapped, clamp(sampled.a, 0.0, 1.0));
}
