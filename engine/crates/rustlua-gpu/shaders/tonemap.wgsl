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
    // P3-R166：相机裁剪平面（世界空间，xyz 法线 + w 偏移）
    section_plane: vec4<f32>,
    // 世界 → 地形局部（x/z 是网格本地坐标，剖面高度图按它索引）
    world_to_local: mat4x4<f32>,
    // x = 高度图最小 y，y = 高度跨度，z = 地形半宽，w = 开关（0 = 关）
    section_terrain: vec4<f32>,
    // P3-R170：x = 平面反射开关，y = 强度，zw 预留
    planar: vec4<f32>,
    // P3-R171：xyz = 水体每米消光系数（线性 RGB），w = 强度（0 = 关）
    water_abs: vec4<f32>,
    // P3-R172：xyz = 水体散射色（线性 RGB），w = 强度（0 = 关）
    water_scatter: vec4<f32>,
    // P3-R173：x = 相机入水开关，y = 光程上限（米）；
    // P3-R207：z = 折射基准强度（0 = 用旧常数 0.0035），w = 深度增益（uv / 米 / 掠射）
    water_under: vec4<f32>,
    // P3-R174：上一帧的视图投影（把当前世界点投回上一帧屏幕）
    previous_view_projection: mat4x4<f32>,
    // x = 强度（0 = 关），y = 屏幕速度上限（uv），zw 预留；w = 开关
    motion: vec4<f32>,
    // P3-R208：x = 水下界面开关（相机入水且档位 > 0），y = 水 → 空气的折射率比（1/1.333），
    // z = 界面替换强度（0 = 关），w = 临界角过渡带宽（余弦值）
    submersion: vec4<f32>,
    // P3-R241：x = 水下屏幕空间步进次数（0 = 关 = R207 的偏移近似），y = 空气 → 水的折射率比
    // （1/1.333 ≈ 0.7519），z = 首步步长（米），w = 命中过冲的厚度容差（米）
    water_refract: vec4<f32>,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var depth_texture: texture_depth_2d;
@group(0) @binding(4) var ao_map: texture_2d<f32>;
@group(0) @binding(5) var ao_sampler: sampler;
@group(0) @binding(6) var ssr_map: texture_2d<f32>;
@group(0) @binding(7) var ssr_sampler: sampler;
// P3-R166：`sect=1` 地形的高度图（RG 两个字节 = 16 位定点）。
@group(0) @binding(8) var section_height: texture_2d<f32>;
// P3-R170：水面平面反射的离屏色图（半分辨率，alpha = 该像素是否有镜像几何）。
@group(0) @binding(9) var planar_map: texture_2d<f32>;
@group(0) @binding(10) var planar_sampler: sampler;

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

// 高度图双线性解码：R = 高 8 位、G = 低 8 位，合成 16 位定点再映射回 [min_y, min_y+span]。
fn section_height_texel(coord: vec2<i32>) -> f32 {
    let last = vec2<i32>(textureDimensions(section_height)) - vec2<i32>(1);
    let texel = textureLoad(section_height, clamp(coord, vec2<i32>(0), last), 0);
    let fixed = texel.r * 255.0 * 256.0 + texel.g * 255.0;
    return params.section_terrain.x + (fixed / 65535.0) * params.section_terrain.y;
}

fn section_height_at(local: vec2<f32>) -> f32 {
    let size = max(params.section_terrain.z * 2.0, 1e-3);
    let uv = local / size + vec2<f32>(0.5);
    let coord = uv * vec2<f32>(textureDimensions(section_height)) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(coord));
    let f = coord - floor(coord);
    let h00 = section_height_texel(base);
    let h10 = section_height_texel(base + vec2<i32>(1, 0));
    let h01 = section_height_texel(base + vec2<i32>(0, 1));
    let h11 = section_height_texel(base + vec2<i32>(1, 1));
    return mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y);
}

// P3-R166：裁剪平面处的实体土壤剖面。相机级裁剪把近侧地形整块丢弃后，
// 屏幕上留下一个能看见地下空洞的缺口；这里用像素射线与裁剪平面求交，
// 再查高度图判断交点是否落在土块内部，是则画程序化层理封口。
// 与视角无关：环绕时只改平面参数，不需要重建任何几何。
fn section_cut(uv: vec2<f32>) -> vec4<f32> {
    if params.section_terrain.w < 0.5 {
        return vec4<f32>(0.0);
    }
    let plane_len = length(params.section_plane.xyz);
    if plane_len < 1e-6 {
        return vec4<f32>(0.0);
    }
    let normal = params.section_plane.xyz / plane_len;
    let offset = params.section_plane.w / plane_len;
    let origin = params.camera_position.xyz;
    let far = world_from_depth(uv, 1.0);
    let direction = normalize(far - origin);
    let denominator = dot(normal, direction);
    if abs(denominator) < 1e-5 {
        return vec4<f32>(0.0);
    }
    let t = -(dot(normal, origin) + offset) / denominator;
    if t <= 0.0 {
        return vec4<f32>(0.0);
    }
    let world = origin + direction * t;
    // 交点必须比场景几何更近：被道具或远侧地形挡住的像素不画剖面。
    let clip = params.view_projection * vec4<f32>(world, 1.0);
    if clip.w <= 1e-4 {
        return vec4<f32>(0.0);
    }
    let ndc = clip.xyz / clip.w;
    if ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 {
        return vec4<f32>(0.0);
    }
    let scene = depth_at(uv);
    if scene < 0.9999 && ndc.z >= scene - 0.0005 {
        return vec4<f32>(0.0);
    }
    let local = (params.world_to_local * vec4<f32>(world, 1.0)).xyz;
    let half_extent = params.section_terrain.z;
    if abs(local.x) > half_extent || abs(local.z) > half_extent {
        return vec4<f32>(0.0);
    }
    let ground = section_height_at(local.xz);
    // 只丢水面以上的交点；交点低于地形最低点也要继续填——土块是实心的，
    // 否则切面会在 `min_y` 处被截成一条窄带，下面重新露出天空。
    if world.y > ground {
        return vec4<f32>(0.0);
    }
    let depth_below = ground - world.y;
    // 层理按世界 y 分层：地质层是水平的，切面离镜头很近时 depth_below 动辄几十单位，
    // 按深度分层会全部饱和成一片纯色；用低频噪声让层线随地形轻微起伏。
    let wobble = tonemap_value_noise3(vec3<f32>(local.x * 0.09, local.z * 0.09, 3.7)) * 1.7;
    let strata = sin((world.y + wobble) * 2.1) * 0.5 + 0.5;
    var soil = mix(vec3<f32>(0.21, 0.15, 0.09), vec3<f32>(0.37, 0.27, 0.16), strata);
    // 钙积层：更浅、更灰的一层，和上下黏土拉开对比。
    soil = mix(soil, vec3<f32>(0.34, 0.30, 0.24), smoothstep(0.72, 0.98, strata) * 0.7);
    let grit = tonemap_value_noise3(world * 1.7) * 0.10
        + tonemap_value_noise3(world * vec3<f32>(4.1, 2.3, 4.7)) * 0.05;
    soil = soil + vec3<f32>(grit * 0.35, grit * 0.30, grit * 0.24);
    // 顶端压一条草皮色：剖面与地表接缝处不出现突兀的亮边。
    soil = mix(vec3<f32>(0.07, 0.13, 0.045), soil, smoothstep(0.02, 0.25, depth_below));
    var shade = 0.55;
    let sun = params.light_direction.xyz;
    if dot(sun, sun) > 1e-8 {
        shade = 0.55 + 0.45 * clamp(dot(normal, normalize(sun)), 0.0, 1.0);
    }
    // 越往深部环境光越弱（低成本 AO），剖面自然变暗。
    shade = shade * mix(1.0, 0.55, clamp(depth_below / max(params.section_terrain.y, 1e-3), 0.0, 1.0));
    return vec4<f32>(soil * shade * 1.35, 1.0);
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
    return sky_along(ray, uv);
}

// P3-R208：天空按**方向**取色。水下视角要把折射出射的方向送进来，
// 所以从 `sky_color` 里拆出这一段；`sky_color` 只剩「由 uv 重建射线」的外壳。
fn sky_along(ray: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
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

// P3-R241：从水面像素出发，沿**Snell 折射方向**在水下做屏幕空间步进，找射线真正钻到
// 几何背面的那一点（返回 `(uv, 命中)`；未命中返回调用方给的偏移落点，保证不会采到黑）。
//
// 口径：射线从相机 → 河床点就是该像素的视线，水面交点由 `y = level` 解析求交得到；
// 入射方向过 `refract(I, N, eta)`（空气 → 水，eta = n_air / n_water）得到水下方向。
// 每步都投影回屏幕、与深度图反投影出的场景点比**到相机的距离**：射线点比场景点远
// 就是已经钻到背面；过冲超过 `water_refract.w` 视为穿过薄物/缝隙，直接放弃（回退）。
// 不做法线空间细化——步长够细时误差落在一个像素内，代价只有 N 次深度取样。
fn refraction_step_uv(bed: vec3<f32>, slope: vec2<f32>, fallback: vec2<f32>) -> vec3<f32> {
    let origin = params.camera_position.xyz;
    let direction = normalize(bed - origin);
    if abs(direction.y) < 1e-4 {
        return vec3<f32>(fallback, 0.0);
    }
    let t_plane = (params.water.x - origin.y) / direction.y;
    if t_plane <= 0.0 {
        return vec3<f32>(fallback, 0.0);
    }
    let hit = origin + direction * t_plane;
    // 与 `water_surface` / 反射同一条波法线口径（-slope × 0.16, 1, -slope × 0.16）。
    let normal = normalize(vec3<f32>(-slope.x * 0.16, 1.0, -slope.y * 0.16));
    let refracted = refract(direction, normal, params.water_refract.y);
    let steps = i32(clamp(params.water_refract.x, 1.0, 32.0));
    let step = max(params.water_refract.z, 0.05);
    let thickness = max(params.water_refract.w, 0.02);
    var march = hit + refracted * step;
    for (var i: i32 = 0; i < steps; i = i + 1) {
        let clip = params.view_projection * vec4<f32>(march, 1.0);
        if clip.w <= 0.0 {
            break;
        }
        let ndc = clip.xyz / clip.w;
        if abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0 {
            break;
        }
        let sample_uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        let sample_depth = depth_at(sample_uv);
        if sample_depth < 0.9999 {
            let scene = world_from_depth(sample_uv, sample_depth);
            let scene_distance = length(scene - origin);
            let march_distance = length(march - origin);
            if scene_distance < march_distance {
                if march_distance - scene_distance <= thickness {
                    return vec3<f32>(sample_uv, 1.0);
                }
                break;
            }
        }
        march = march + refracted * (step * (1.0 + f32(i) * 0.35));
    }
    return vec3<f32>(fallback, 0.0);
}

fn water_refraction(uv: vec2<f32>, time: f32, optical_depth: f32, grazing: f32) -> vec3<f32> {
    let surface_depth = depth_at(uv);
    let world = world_from_depth(uv, surface_depth);
    let slope = tonemap_water_slope(world.xz, time);
    // P3-R207：折射偏移的强度。`water_under.z`（基准）为 0 = 旧行为，用下面的常数；
    // 开档后基准由宿主给出，再按「逐像素光程 × 掠射角」放大——水越深、视角越平，
    // 视差位移越大。P3-R241 起它退化为**回退落点**：步进命中的像素改采命中点，
    // 步数档为 0（默认）时一个字节都不变。
    let strength = select(
        0.0035,
        params.water_under.z + params.water_under.w * optical_depth * grazing,
        params.water_under.z > 0.0,
    );
    let offset = vec2<f32>(slope.x, slope.y) * strength;
    // P3-R241：开档后先走 Snell 步进；`stepped.z` 是命中掩码。没命中就保留偏移落点
    // （深水/掠射/出屏时步进找不到交点，此时退回近似采样而不是采黑）。
    var hit_uv = uv + offset;
    var hit = false;
    if params.water_refract.x >= 0.5 && surface_depth < 0.9999 {
        let stepped = refraction_step_uv(world, slope, hit_uv);
        if stepped.z > 0.5 {
            hit_uv = stepped.xy;
            hit = true;
        }
    }
    // 轻微色散：R/B 比 G 多/少一点偏移，水面边缘有很细的彩边。步进命中时三个通道采
    // 同一个命中点（真实折射的色散量远小于一个像素，不在这里伪造彩边）。
    let r_uv = select(uv + offset * 1.08, hit_uv, hit);
    let g_uv = select(uv + offset, hit_uv, hit);
    let b_uv = select(uv + offset * 0.92, hit_uv, hit);
    let r = textureSample(source, source_sampler, clamp(r_uv, vec2<f32>(0.001), vec2<f32>(0.999))).r;
    let g = textureSample(source, source_sampler, clamp(g_uv, vec2<f32>(0.001), vec2<f32>(0.999))).g;
    let b = textureSample(source, source_sampler, clamp(b_uv, vec2<f32>(0.001), vec2<f32>(0.999))).b;
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

struct WaterSurface {
    hit: vec3<f32>,
    // 该像素深度反投影出的世界点：水面是 blend、不写深度，所以水像素上它就是河床。
    bed: vec3<f32>,
    // 相机 → 该像素的方向（反射步进与菲涅尔都要用）。
    direction: vec3<f32>,
    valid: bool,
};

// 水像素判定（P3-R171 起三处共用）：射线打到 `y = level` 平面、落点在水面矩形内，
// 且该像素的深度在水面之下。天空（depth = 1）与岸上像素一律为 false —— 它们没有河床，
// 拿它们的深度算光程会把整屏压黑。
fn water_surface(uv: vec2<f32>) -> WaterSurface {
    var surface: WaterSurface;
    surface.hit = vec3<f32>(0.0);
    surface.bed = vec3<f32>(0.0);
    surface.direction = vec3<f32>(0.0, -1.0, 0.0);
    surface.valid = false;
    if params.water_params.x < 0.5 {
        return surface;
    }
    let surface_depth = depth_at(uv);
    if surface_depth >= 0.9999 {
        return surface;
    }
    let level = params.water.x;
    let half = params.water.y;
    let center = vec2<f32>(params.water.z, params.water.w);
    let far = world_from_depth(uv, 1.0);
    let direction = normalize(far - params.camera_position.xyz);
    if abs(direction.y) < 1e-4 {
        return surface;
    }
    let t = (level - params.camera_position.y) / direction.y;
    if t <= 0.0 {
        return surface;
    }
    let hit = params.camera_position.xyz + direction * t;
    if abs(hit.x - center.x) > half || abs(hit.z - center.y) > half {
        return surface;
    }
    let bed = world_from_depth(uv, surface_depth);
    if bed.y > level - 0.01 {
        return surface;
    }
    surface.hit = hit;
    surface.bed = bed;
    surface.direction = direction;
    surface.valid = true;
    return surface;
}

// 屏幕空间水面反射（P3-R52）：用解析水面求交定位水像素，再以屏幕空间步进找
// 水面之上的场景颜色。只在水面像素且反射命中时返回非零，避免影响 UI/地形。
// P3-R173：相机在水下时的体积吸收。与 P3-R171 的区别是光程取「相机 → 该像素世界点」
// 的距离（整帧都隔着水），而不是水像素的河床深度；因此只在相机入水时开。天空像素
// depth = 1 反投影到远点，distance 被 `water_under.y` 截断，不会把整帧压成纯水体色。
fn underwater_transmittance(uv: vec2<f32>) -> vec3<f32> {
    let point = world_from_depth(uv, depth_at(uv));
    let distance = clamp(length(point - params.camera_position.xyz), 0.0, params.water_under.y);
    return exp(-params.water_abs.rgb * distance * params.water_abs.w);
}

struct SubmergedInterface {
    // 相机 → 界面命中点的光程（米）。界面像素的光程是它，不是几何体的距离。
    path: f32,
    // 折射窗口权重：1 = 透过界面看到界外的天空，0 = 临界角外（全反射看水下）。
    window: f32,
    // 临界环亮度：窗口边缘处入射角最接近临界角，那里聚光最亮。
    ring: f32,
    // 出射方向（折射后）；WGSL 的 `refract` 在全反射时退化成 `reflect`，正好给镜像方向。
    direction: vec3<f32>,
    // 该像素是否真的穿过水-气界面：射线朝上、落点在水面矩形内、且几何体没挡在界面之前。
    through: bool,
};

// P3-R208：从水下看水-气界面。相机在水位以下时，向上看的像素原本只会跟着
// P3-R173 的整帧吸收一起变暗 —— 但真实水下视角的招牌形状是「Snell 窗口」：
// 临界角（水 → 空气约 48.6°）以内能看到折射出去的天空，角外是全反射的银面。
// 这里按解析平面求交复用 P3-R171 那份水面矩形，不新增几何。
fn submerged_interface(uv: vec2<f32>) -> SubmergedInterface {
    var view: SubmergedInterface;
    view.path = 0.0;
    view.window = 0.0;
    view.ring = 0.0;
    view.direction = vec3<f32>(0.0, 1.0, 0.0);
    view.through = false;
    if params.submersion.x < 0.5 {
        return view;
    }
    let origin = params.camera_position.xyz;
    let far = world_from_depth(uv, 1.0);
    let incident = normalize(far - origin);
    // 朝下看的像素永远打不到界面（看到的是河床、道具、落水的蚁）⇒ 原样不动。
    if incident.y <= 1e-4 {
        return view;
    }
    let t = (params.water.x - origin.y) / incident.y;
    if t <= 0.0 {
        return view;
    }
    let hit = origin + incident * t;
    let half = params.water.y;
    let center = vec2<f32>(params.water.z, params.water.w);
    if abs(hit.x - center.x) > half || abs(hit.z - center.y) > half {
        return view;
    }
    // 界面之前有几何体 ⇒ 这个像素看的不是界面。水面是 blend、不写深度，所以
    // 只有真正画了东西的像素才有深度；天空（depth = 1）总是穿过界面。
    let surface_depth = depth_at(uv);
    if surface_depth < 0.9999 {
        let geometry = world_from_depth(uv, surface_depth);
        if length(geometry - origin) <= t {
            return view;
        }
    }
    let slope = tonemap_water_slope(hit.xz, params.water_params.y);
    // 水面向上的法线（与 P3-R52 屏幕空间反射同一份波斜率与振幅口径）。
    let normal = normalize(vec3<f32>(-slope.x * 0.16, 1.0, -slope.y * 0.16));
    let eta = params.submersion.y;
    let cosine = dot(incident, normal);
    // 临界角的余弦：cos θc = sqrt(1 − η²)，η = 1/1.333 ⇒ θc ≈ 48.6°。
    let critical = sqrt(max(1.0 - eta * eta, 0.0));
    let band = max(params.submersion.w, 1e-3);
    view.path = t;
    view.window = smoothstep(critical - band, critical + band, cosine);
    view.ring = exp(-pow((cosine - critical) / band, 2.0));
    view.direction = refract(incident, -normal, eta);
    view.through = true;
    return view;
}

// 界面像素的辐射：窗口内取折射出射方向上的天空（含体积云），窗口外是全反射回
// 水下的暗银面 + 临界环聚光。两者都还要再过一次「相机 → 界面」的水体吸收。
fn submerged_surface_color(view: SubmergedInterface, uv: vec2<f32>) -> vec3<f32> {
    let above = sky_along(view.direction, uv);
    let body = params.water_scatter.rgb * params.water_scatter.w;
    // 全反射侧看不到界外：给一层比纯水色亮的水下镜像 + 临界环，
    // 这就是水下照片里那一圈围着窗口的银亮边。
    let mirror = body * 1.8 + vec3<f32>(0.055, 0.075, 0.095) * view.ring;
    return mix(mirror, above, view.window);
}

// P3-R208：水下像素的最终辐射。穿过界面的像素换成窗口/全反射镜像，光程改成
// 「相机 → 界面」；其余像素维持 P3-R173 的原式。开关为 0（默认）时恒走原式
// ⇒ 已有的水下画面逐字节不变。
fn underwater_radiance(uv: vec2<f32>, linear: vec3<f32>) -> vec3<f32> {
    let body = params.water_scatter.rgb * params.water_scatter.w;
    if params.submersion.x > 0.5 {
        let view = submerged_interface(uv);
        if view.through {
            let path = min(view.path, params.water_under.y);
            let transmittance = exp(-params.water_abs.rgb * path * params.water_abs.w);
            let surface = submerged_surface_color(view, uv);
            return mix(linear, surface, params.submersion.z) * transmittance
                + body * (1.0 - transmittance);
        }
    }
    let transmittance = underwater_transmittance(uv);
    return linear * transmittance + body * (1.0 - transmittance);
}

fn water_reflection(uv: vec2<f32>, surface: WaterSurface) -> vec3<f32> {
    if !surface.valid {
        return vec3<f32>(0.0);
    }
    let hit = surface.hit;
    let slope = tonemap_water_slope(hit.xz, params.water_params.y);
    let normal = normalize(vec3<f32>(-slope.x * 0.16, 1.0, -slope.y * 0.16));
    let reflected = reflect(surface.direction, normal);
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
        let sample_depth = depth_at(sample_uv);
        if sample_depth >= 0.9999 {
            continue;
        }
        let scene = world_from_depth(sample_uv, sample_depth);
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

// P3-R170：水面平面反射。镜像相机把水面之上的不透明场景画进 `planar_map`。
// 镜像矩阵是 `VP·R`（没有裁剪空间 x 取反），镜像图已经是屏幕朝向：水面点关于
// `y = level` 镜像后仍在同一屏幕位置，因此当前像素的 uv 就是反射图的采样坐标。
// 只在水面像素（解析求交 + 河床深度守卫）采样；alpha 是"该像素有没有镜像几何"的掩码，
// 没有就返回零，由调用方回退到屏幕空间反射。
fn planar_reflection(uv: vec2<f32>, surface: WaterSurface) -> vec3<f32> {
    if params.planar.x < 0.5 {
        return vec3<f32>(0.0);
    }
    if !surface.valid {
        return vec3<f32>(0.0);
    }
    let hit = surface.hit;
    let slope = tonemap_water_slope(hit.xz, params.water_params.y);
    let normal = normalize(vec3<f32>(-slope.x * 0.16, 1.0, -slope.y * 0.16));
    let texel = 1.0 / vec2<f32>(textureDimensions(planar_map));
    let base = clamp(
        uv + normal.xz * 0.004,
        vec2<f32>(0.0),
        vec2<f32>(1.0),
    );
    // 5 抽头：中心权重最高，四周柔化半分辨率反射图的锯齿。
    let c0 = textureSampleLevel(planar_map, planar_sampler, base, 0.0);
    let c1 = textureSampleLevel(planar_map, planar_sampler, base + texel * vec2<f32>(2.0, 0.0), 0.0);
    let c2 = textureSampleLevel(planar_map, planar_sampler, base - texel * vec2<f32>(2.0, 0.0), 0.0);
    let c3 = textureSampleLevel(planar_map, planar_sampler, base + texel * vec2<f32>(0.0, 2.0), 0.0);
    let c4 = textureSampleLevel(planar_map, planar_sampler, base - texel * vec2<f32>(0.0, 2.0), 0.0);
    let weight = c0.a * 0.40 + c1.a * 0.15 + c2.a * 0.15 + c3.a * 0.15 + c4.a * 0.15;
    if weight < 0.01 {
        return vec3<f32>(0.0);
    }
    let color = (c0.rgb * c0.a * 0.40
        + c1.rgb * c1.a * 0.15
        + c2.rgb * c2.a * 0.15
        + c3.rgb * c3.a * 0.15
        + c4.rgb * c4.a * 0.15) / weight;
    let view = normalize(params.camera_position.xyz - hit);
    let fresnel = pow(1.0 - clamp(dot(normal, view), 0.0, 1.0), 5.0);
    return color * (0.25 + fresnel * 0.65) * params.planar.y;
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

// P3-R175：运动模糊的单个抽头。源图里天空像素只有清屏色，最终颜色是后面 `sky_color` 现算的；
// 抽头必须走同一条替换，否则地面像素会把清屏色混进来，在地平线压出一条本来不存在的暗边
// （P3-R174 的已知代价，本版修掉）。
fn motion_tap(uv: vec2<f32>, gain: f32) -> vec3<f32> {
    if depth_at(uv) >= 0.9999 && params.sky.x > 0.5 {
        return sky_color(uv);
    }
    return textureSampleLevel(source, source_sampler, uv, 0.0).rgb * gain;
}

// P3-R174：相机运动模糊。把当前像素的世界点用上一帧的视图投影投回屏幕，
// 与当前 uv 的差就是这一像素一帧内扫过的屏幕距离；沿这个方向取 4 个抽头。
// 相机静止 ⇒ 重投影回同一个 uv ⇒ 速度为零 ⇒ 原样返回（帧逐字节不变）。
// 天空像素没有可信深度，跳过。抽头按当前深度边界直接采，不做邻域深度剔除：
// 动态物体的拖影边缘会带一点鬼影，属于这一档 "lite" 的已知代价。
fn motion_blur_at(uv: vec2<f32>, center: vec3<f32>, gain: f32) -> vec3<f32> {
    if params.motion.w < 0.5 || params.motion.x <= 0.0 {
        return center;
    }
    let depth = depth_at(uv);
    if depth >= 0.9999 {
        return center;
    }
    let point = world_from_depth(uv, depth);
    let homogeneous = params.previous_view_projection * vec4<f32>(point, 1.0);
    if homogeneous.w <= 1e-6 {
        return center;
    }
    let ndc = homogeneous.xy / homogeneous.w;
    let previous_uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    let velocity = (uv - previous_uv) * params.motion.x;
    if dot(velocity, velocity) < 1e-12 {
        return center;
    }
    // 上限挡住"相机瞬移后整屏抽到另一侧"的情形（重投影在遮挡边界会给出巨大位移）。
    let limit = max(params.motion.y, 1e-4);
    let offset = clamp(velocity, vec2<f32>(-limit), vec2<f32>(limit));
    var accumulation = center;
    var weight = 1.0;
    for (var i = 1; i <= 4; i = i + 1) {
        let t = f32(i) * 0.25;
        let tap_weight = 1.0 - t * 0.5;
        let forward = clamp(uv + offset * t, vec2<f32>(0.0), vec2<f32>(1.0));
        let backward = clamp(uv - offset * t, vec2<f32>(0.0), vec2<f32>(1.0));
        accumulation += motion_tap(forward, gain) * tap_weight;
        accumulation += motion_tap(backward, gain) * tap_weight;
        weight += tap_weight * 2.0;
    }
    return accumulation / weight;
}

@fragment fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let sampled = textureSample(source, source_sampler, uv);
    let gain = exp2(params.exposure_ev);
    var linear = max(sampled.rgb * gain, vec3<f32>(0.0));
    // P3-R174：在泛光之前做，拖影不参与 HDR 泛光的扩散（避免整屏泛白）。
    linear = motion_blur_at(uv, linear, gain);
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
        // P3-R171：只有水像素才合成折射/吸收/反射。天空与岸上像素的深度不是河床，
        // 用它们算光程会把整屏压黑（折射过去是整屏生效的，这里一并收进水掩码）。
        let surface = water_surface(uv);
        if surface.valid {
            // P3-R87：屏幕空间折射——用波斜率扰动场景颜色，河床/水下物体随水面晃动。
            // P3-R171：光学吸收。水面是 blend、不写深度，`surface.bed` 就是河床，
            // 光程 = 水位 − 河床 y；Beer–Lambert 逐通道透射让浅滩透亮、深水收暗。
            // 折射混合之后整体乘一次透射（只乘一次：折射色和水面自身合成色共用同一段光程）。
            let optical_depth = clamp(params.water.x - surface.bed.y, 0.0, 32.0);
            let transmittance = exp(-params.water_abs.rgb * optical_depth * params.water_abs.w);
            let grazing = clamp(1.0 - abs(surface.direction.y), 0.0, 1.0);
            let refracted = water_refraction(uv, params.camera_position.w, optical_depth, grazing);
            // P3-R172：散射项。只有吸收时河床的辐射会被乘到接近 0，深水在画面上收成死黑；
            // 真实水体自己散射出一层水体色 `body`，权重是 `1 − T`，所以越深越接近它、
            // 浅滩几乎不受影响（`T ≈ 1`）。
            let body = params.water_scatter.rgb * params.water_scatter.w;
            linear = mix(linear, refracted, 0.30) * transmittance + body * (1.0 - transmittance);
            // P3-R170：平面反射有真实镜像几何，优先；未命中回退屏幕空间反射。
            let planar = planar_reflection(uv, surface);
            let screen = water_reflection(uv, surface);
            linear = linear + select(screen, planar, length(planar) > 0.001) * 0.65;
        }
    }
    if params.sky.x > 0.5 && depth_at(uv) >= 0.9999 {
        linear = sky_color(uv);
    }
    linear = linear + god_rays_at(uv);
    linear = mix(linear, params.fog.yzw, fog_at(uv));
    // P3-R166：裁剪平面后的实体土壤剖面（只处理 `sect=1` 地形 + 裁剪同时存在的帧）。
    let cut = section_cut(uv);
    if (cut.a > 0.5) {
        linear = cut.rgb;
    }
    // P3-R173：相机入水后的整帧体积吸收 + 散射。放在剖面封口之后、闪白之前，
    // 保证水下的土壤剖面与 UI 之前的整个 3D 画面都过同一段光程。
    // P3-R208：同一分支里再分岔 —— 向上打到水-气界面的像素换成 Snell 窗口。
    if params.water_under.x > 0.5 {
        linear = underwater_radiance(uv, linear);
    }
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
