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
    // x = level，y = 半宽，zw = 中心 xz；water_params.x = 开关
    water: vec4<f32>,
    // x = 开关，y = 时间，z = 反射强度，w = 预留
    water_params: vec4<f32>,
    // x = cloud base height (world y); yzw reserved
    cloud: vec4<f32>,
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

fn tonemap_hash31(p_in: vec3<f32>) -> f32 {
    var p = fract(p_in * 0.1031);
    p = p + dot(p, p.yzx + vec3<f32>(33.33));
    return fract((p.x + p.y) * p.z);
}

fn tonemap_value_noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let n000 = tonemap_hash31(i);
    let n100 = tonemap_hash31(i + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = tonemap_hash31(i + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = tonemap_hash31(i + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = tonemap_hash31(i + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = tonemap_hash31(i + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = tonemap_hash31(i + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = tonemap_hash31(i + vec3<f32>(1.0, 1.0, 1.0));
    let x00 = mix(n000, n100, u.x);
    let x10 = mix(n010, n110, u.x);
    let x01 = mix(n001, n101, u.x);
    let x11 = mix(n011, n111, u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
}

fn tonemap_hash21(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(123.34, 456.21));
    let r = q + dot(q, q + vec2<f32>(45.32));
    return fract(r.x * r.y);
}

fn tonemap_value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = tonemap_hash21(i);
    let b = tonemap_hash21(i + vec2<f32>(1.0, 0.0));
    let c = tonemap_hash21(i + vec2<f32>(0.0, 1.0));
    let d = tonemap_hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// 与 mesh3d_pbr 的地面云影同一片噪声/漂移：地面影与天空云同源（P3-R69）。
fn tonemap_cloud_shadow(world: vec2<f32>, time: f32) -> f32 {
    let drift = vec2<f32>(time * 0.35, time * 0.12);
    let p = world * 0.0035 + drift * 0.05;
    let n = tonemap_value_noise(p) * 0.62
        + tonemap_value_noise(p * 2.3 + vec2<f32>(7.1, 3.7)) * 0.38;
    return smoothstep(0.50, 0.74, n);
}

fn tonemap_cloud_density(p: vec3<f32>, time: f32, coverage: f32, speed: f32) -> f32 {
    let drift = vec3<f32>(time * speed * 0.35, 0.0, time * speed * 0.12);
    let q = (p + drift) * 0.010;
    let n = tonemap_value_noise3(q) * 0.55
        + tonemap_value_noise3(q * 2.1 + vec3<f32>(7.1, 3.7, 1.3)) * 0.30
        + tonemap_value_noise3(q * 4.3 + vec3<f32>(1.7, 9.2, 4.1)) * 0.15;
    let height = clamp((p.y - 90.0) / 140.0, 0.0, 1.0);
    let bias = (clamp(coverage, 0.0, 1.0) - 0.5) * 0.20;
    return smoothstep(0.46 - bias, 0.72 - bias, n)
        * smoothstep(0.0, 0.18, height)
        * (1.0 - smoothstep(0.62, 1.0, height) * 0.35);
}

// 体积云：在 y∈[100,240] 的薄层里步进 8 次，朝太阳再采 2 次做自阴影。
fn tonemap_volumetric_clouds(ray: vec3<f32>, to_sun: vec3<f32>, time: f32, uv: vec2<f32>) -> vec4<f32> {
    let ro = params.camera_position.xyz;
    let base = clamp(params.cloud.x, 40.0, 200.0);
    let top = base + 140.0;
    if (ray.y <= 0.02) {
        return vec4<f32>(0.0);
    }
    let t0 = max((base - ro.y) / max(ray.y, 0.05), 0.0);
    let t1 = (top - ro.y) / max(ray.y, 0.05);
    if (t1 <= t0) {
        return vec4<f32>(0.0);
    }
    let step_count = 8;
    let dt = (t1 - t0) / f32(step_count);
    // P3-R79：逐像素抖动起点，打散 8 步固定采样的条带；哈希随 uv/时间变化。
    let jitter = tonemap_hash21(uv * vec2<f32>(1920.0, 1080.0) + vec2<f32>(time * 37.0, time * 17.0));
    let t0_jittered = t0 + jitter * dt;
    var transmittance = 1.0;
    var accum = vec3<f32>(0.0);
    for (var i: i32 = 0; i < step_count; i = i + 1) {
        let p = ro + ray * (t0_jittered + (f32(i) + 0.5) * dt);
        let density = tonemap_cloud_density(p, time, params.sky.w, params.water_params.w);
        if (density > 0.001) {
            var light = 1.0;
            for (var j: i32 = 1; j <= 2; j = j + 1) {
                light = light - tonemap_cloud_density(p + to_sun * (f32(j) * 18.0), time, params.sky.w, params.water_params.w) * 0.22;
            }
            light = clamp(light, 0.28, 1.0);
            let lit = mix(vec3<f32>(0.50, 0.56, 0.66), vec3<f32>(1.0, 0.97, 0.90), light);
            accum = accum + lit * density * dt * 0.055 * transmittance;
            transmittance = transmittance * (1.0 - density * 0.16);
        }
    }
    let coverage = (1.0 - transmittance) * smoothstep(0.03, 0.20, ray.y);
    return vec4<f32>(accum, coverage);
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
    // P3-R69：体积云，透视步进 + 朝太阳的 2 次自阴影。
    let time = params.camera_position.w;
    let clouds = tonemap_volumetric_clouds(ray, to_sun, time, uv);
    color = color * (1.0 - clouds.a * 0.85) + clouds.rgb;
    return color;
}

fn water_refraction(uv: vec2<f32>, time: f32) -> vec3<f32> {
    let surface_depth = depth_at(uv);
    let world = world_from_depth(uv, surface_depth);
    let slope = tonemap_water_slope(world.xz, time);
    let offset = vec2<f32>(slope.x, slope.y) * 0.0035;
    // 轻微色散：R/B 比 G 多/少一点偏移，水面边缘有很细的彩边。
    let r = textureSample(source, source_sampler, clamp(uv + offset * 1.08, vec2<f32>(0.001), vec2<f32>(0.999))).r;
    let g = textureSample(source, source_sampler, clamp(uv + offset, vec2<f32>(0.001), vec2<f32>(0.999))).g;
    let b = textureSample(source, source_sampler, clamp(uv + offset * 0.92, vec2<f32>(0.001), vec2<f32>(0.999))).b;
    return vec3<f32>(r, g, b);
}

// 水面波斜率（与 mesh3d_pbr 的三条长波同相；Tonemap 只需要近似法线）。
fn tonemap_water_slope(p: vec2<f32>, time: f32) -> vec2<f32> {
    let advance = time * 0.9;
    let k1 = vec2<f32>(0.62, 0.31);
    let k2 = vec2<f32>(-0.28, 0.74);
    let k3 = vec2<f32>(1.35, -0.85);
    let ph1 = dot(p, k1) + advance * 1.3;
    let ph2 = dot(p, k2) - advance * 1.7;
    let ph3 = dot(p, k3) + advance * 2.4;
    return vec2<f32>(
        cos(ph1) * k1.x * 0.55 + cos(ph2) * k2.x * 0.35 + cos(ph3) * k3.x * 0.16,
        cos(ph1) * k1.y * 0.55 + cos(ph2) * k2.y * 0.35 + cos(ph3) * k3.y * 0.16
    );
}

// 屏幕空间水面反射（P3-R52）：用解析水面求交定位水像素，再以屏幕空间步进找
// 水面之上的场景颜色。只在水面像素且反射命中时返回非零，避免影响 UI/地形。
fn water_reflection(uv: vec2<f32>) -> vec3<f32> {
    if params.water_params.x < 0.5 {
        return vec3<f32>(0.0);
    }
    let surface_depth = depth_at(uv);
    if surface_depth >= 0.9999 {
        return vec3<f32>(0.0);
    }
    let level = params.water.x;
    let half = params.water.y;
    let center = vec2<f32>(params.water.z, params.water.w);
    let far = world_from_depth(uv, 1.0);
    let direction = normalize(far - params.camera_position.xyz);
    if abs(direction.y) < 1e-4 {
        return vec3<f32>(0.0);
    }
    let t = (level - params.camera_position.y) / direction.y;
    if t <= 0.0 {
        return vec3<f32>(0.0);
    }
    let hit = params.camera_position.xyz + direction * t;
    if abs(hit.x - center.x) > half || abs(hit.z - center.y) > half {
        return vec3<f32>(0.0);
    }
    let bed = world_from_depth(uv, surface_depth);
    if bed.y > level - 0.01 {
        return vec3<f32>(0.0);
    }
    let slope = tonemap_water_slope(hit.xz, params.water_params.y);
    let normal = normalize(vec3<f32>(-slope.x * 0.16, 1.0, -slope.y * 0.16));
    let reflected = reflect(direction, normal);
    if reflected.y <= 0.0 {
        return vec3<f32>(0.0);
    }
    var point = hit + vec3<f32>(0.0, 0.02, 0.0);
    var color = vec3<f32>(0.0);
    var found = false;
    for (var step: i32 = 0; step < 18; step = step + 1) {
        point = point + reflected * (1.6 + f32(step) * 0.85);
        let clip = params.view_projection * vec4<f32>(point, 1.0);
        if clip.w <= 0.0 {
            break;
        }
        let ndc = clip.xyz / clip.w;
        if abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0 {
            break;
        }
        let sample_uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        let surface = depth_at(sample_uv);
        if surface >= 0.9999 {
            continue;
        }
        let scene = world_from_depth(sample_uv, surface);
        let difference = point.y - scene.y;
        if difference < 0.0 && difference > -3.5 {
            // 水面反射不是镜子：沿法线做一点 UV 扰动，再取 3 抽头柔化。
            let texel = 1.0 / vec2<f32>(textureDimensions(source));
            let distorted = clamp(
                sample_uv + normal.xz * 0.0035,
                vec2<f32>(0.0),
                vec2<f32>(1.0),
            );
            color = textureSample(source, source_sampler, distorted).rgb * 0.40
                + textureSample(source, source_sampler, distorted + texel * vec2<f32>(2.0, 0.0)).rgb * 0.15
                + textureSample(source, source_sampler, distorted - texel * vec2<f32>(2.0, 0.0)).rgb * 0.15
                + textureSample(source, source_sampler, distorted + texel * vec2<f32>(0.0, 2.0)).rgb * 0.15
                + textureSample(source, source_sampler, distorted - texel * vec2<f32>(0.0, 2.0)).rgb * 0.15;
            found = true;
            break;
        }
    }
    if !found {
        return vec3<f32>(0.0);
    }
    let view = normalize(params.camera_position.xyz - hit);
    let fresnel = pow(1.0 - clamp(dot(normal, view), 0.0, 1.0), 5.0);
    return color * (0.20 + fresnel * 0.60) * params.water_params.z;
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
    let base = 1.0 - exp(-params.fog.x * distance);
    // 低空湿雾（P3-R47）：高度 0.5 以下最强、随高度指数衰减；只在远景渐入，
    // 近处草甸保持对比度，边缘/河岸远处有一层薄雾。
    let ground = exp(-max(world.y - 0.5, 0.0) * 0.35);
    let far = smoothstep(40.0, 220.0, distance);
    let mist = (1.0 - exp(-distance * 0.003)) * ground * 0.18 * far;
    return clamp(base + mist, 0.0, 0.94);
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

fn output_dither(uv: vec2<f32>) -> f32 {
    let p = floor(uv * vec2<f32>(textureDimensions(source)));
    let h = fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
    return (h - 0.5) / 255.0;
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
    if params.water_params.x > 0.5 {
        // P3-R87：屏幕空间折射——用波斜率扰动场景颜色，河床/水下物体随水面晃动。
        let refracted = water_refraction(uv, params.camera_position.w);
        linear = mix(linear, refracted, 0.30);
        linear = linear + water_reflection(uv) * 0.65;
    }
    if params.sky.x > 0.5 && depth_at(uv) >= 0.9999 {
        linear = sky_color(uv);
    }
    linear = linear + god_rays_at(uv);
    linear = mix(linear, params.fog.yzw, fog_at(uv));
    // P3-R86：暴雨闪电的屏幕闪光，Tonemap 前叠加，避免被曝光压掉。
    linear = linear + vec3<f32>(0.72, 0.80, 1.0) * params.cloud.y * 0.55;
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
    // P3-R140：可选暗角/颗粒（params.cloud.z/w），默认 0。
    if params.cloud.z > 0.001 {
        let edge = length(uv - vec2<f32>(0.5)) * 1.414;
        mapped = mapped * (1.0 - params.cloud.z * smoothstep(0.55, 1.0, edge));
    }
    if params.cloud.w > 0.001 {
        let px = floor(uv * vec2<f32>(textureDimensions(source)));
        let n = fract(sin(dot(px, vec2<f32>(12.9898, 78.233))) * 43758.5453);
        mapped = clamp(mapped + vec3<f32>((n - 0.5) * params.cloud.w * 0.08), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    // P3-R102：8-bit 输出抖动，打散天空/雾/水面的渐变条带。
    mapped = clamp(mapped + vec3<f32>(output_dither(uv)), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(mapped, clamp(sampled.a, 0.0, 1.0));
}
