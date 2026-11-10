// 半分辨率 SSR-lite：从深度重建世界坐标与法线，沿反射方向做 10 步定长推进，
// 命中后取 HDR 场景颜色并按菲涅耳衰减。输出 Rgba16Float（rgb=反射，a=命中）。
struct Params {
    inverse_view_projection: mat4x4<f32>,
    view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
    // P3-R176：xyz = 指向太阳的单位向量，w = 太阳项强度（0 ⇒ 天空回退整段关闭）
    sky: vec4<f32>,
    // P3-R178：x = 云量（0..1）、y = 云速、z = 时间、w = 云底高度（世界 y）
    clouds: vec4<f32>,
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

// 命中与天空回退共用的反射权重：掠射角菲涅耳，正对观察时只有一层很弱的底噪。
fn reflection_weight(normal: vec3<f32>, view: vec3<f32>) -> f32 {
    let fresnel = pow(1.0 - clamp(dot(normal, view), 0.0, 1.0), 3.0);
    return 0.12 + 0.88 * fresnel;
}

// P3-R176：未命中/出屏的方向回退。此前两条未命中路径都返回零，反射只能是黑的；
// 这里按反射方向取程序化天空的同一组渐变常量（天顶 0.055/0.14/0.34、地平线
// 0.55/0.68/0.80、指数 0.55、地平线以下 0.34/0.38/0.41，与 Tonemap 的 `sky_color` 一致），
// 太阳项复用 `params.sky.w`（宿主在天空关闭时写 0，整段回退随之失效）。
// 不含体积云与云遮挡：半分辨率 + 3×3 双边之后看不出来，是这一档明写的代价。
// P3-R177：反射里的太阳项上限（乘 `params.sky.w` 之前）。1400 次幂的盘心只有几像素宽，
// 半分辨率 + 3×3 双边会把它摊成一大片白；夹到 0.25 后整条太阳项 ≤ 0.25×2.4 = 0.6，
// 乘 `reflection_weight` 与 Tonemap 的 0.45 后仍打不饱和 LDR —— 日晕与盘心提示都在，
// 代价是反射里的太阳盘比 Tonemap 直出的暗。
const SUN_REFLECTION_CAP: f32 = 0.25;

// P3-R178：与 Tonemap 的体积云同一片噪声、同一覆盖率/云速/云底（函数体逐字相同，
// 只改了名字前缀）。反射的天空里没有云就会与直出天空对不上：同一个方向，抬头是云、
// 低头看水面/地面反而一片干净渐变。
fn ssr_hash31(p_in: vec3<f32>) -> f32 {
    var p = fract(p_in * 0.1031);
    p = p + dot(p, p.yzx + vec3<f32>(33.33));
    return fract((p.x + p.y) * p.z);
}

fn ssr_value_noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let n000 = ssr_hash31(i);
    let n100 = ssr_hash31(i + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = ssr_hash31(i + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = ssr_hash31(i + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = ssr_hash31(i + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = ssr_hash31(i + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = ssr_hash31(i + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = ssr_hash31(i + vec3<f32>(1.0, 1.0, 1.0));
    let x00 = mix(n000, n100, u.x);
    let x10 = mix(n010, n110, u.x);
    let x01 = mix(n001, n101, u.x);
    let x11 = mix(n011, n111, u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
}

fn ssr_cloud_density(p: vec3<f32>, time: f32, coverage: f32, speed: f32) -> f32 {
    let drift = vec3<f32>(time * speed * 0.35, 0.0, time * speed * 0.12);
    let q = (p + drift) * 0.010;
    let n = ssr_value_noise3(q) * 0.55
        + ssr_value_noise3(q * 2.1 + vec3<f32>(7.1, 3.7, 1.3)) * 0.30
        + ssr_value_noise3(q * 4.3 + vec3<f32>(1.7, 9.2, 4.1)) * 0.15;
    let height = clamp((p.y - 90.0) / 140.0, 0.0, 1.0);
    let bias = (clamp(coverage, 0.0, 1.0) - 0.5) * 0.20;
    return smoothstep(0.46 - bias, 0.72 - bias, n)
        * smoothstep(0.0, 0.18, height)
        * (1.0 - smoothstep(0.62, 1.0, height) * 0.35);
}

fn sky_reflection(
    normal: vec3<f32>,
    view: vec3<f32>,
    direction: vec3<f32>,
    world: vec3<f32>,
) -> vec4<f32> {
    if params.sky.w <= 0.0 || direction.y <= 0.0 {
        return vec4<f32>(0.0);
    }
    let height = clamp(direction.y, 0.0, 1.0);
    let zenith = vec3<f32>(0.055, 0.14, 0.34);
    let horizon = vec3<f32>(0.55, 0.68, 0.80);
    var color = mix(horizon, zenith, pow(height, 0.55));
    color = mix(vec3<f32>(0.34, 0.38, 0.41), color, smoothstep(-0.18, 0.01, direction.y));
    let to_sun = params.sky.xyz / max(length(params.sky.xyz), 1e-6);
    let sun = max(dot(direction, to_sun), 0.0);
    let sun_term = pow(sun, 1400.0) * 6.0 + pow(sun, 24.0) * 0.10;
    color = color
        + vec3<f32>(1.0, 0.82, 0.58) * min(sun_term, SUN_REFLECTION_CAP) * params.sky.w;
    // 云遮挡只采一次云底平面（不是 Tonemap 的 8 步体积积分）+ 只做透射衰减，不加云自身
    // 散射色：半分辨率 + 3×3 双边之后多步积分看不出差别。
    if (params.clouds.x > 0.001 && direction.y > 0.02) {
        let t = (clamp(params.clouds.w, 40.0, 200.0) - world.y) / direction.y;
        if (t > 0.0) {
            let density = ssr_cloud_density(
                world + direction * t,
                params.clouds.z,
                params.clouds.x,
                params.clouds.y,
            );
            // 一次散射的相位近似：朝向太阳的云顶亮且偏暖，背光面是冷灰（天光）。
            // 吸收掉的能量按 0.14 权重换回云自己的受光色（P3-R179）——只变暗会让深云区
            // 变成一块比周围更「蓝」的洞，而真实的云顶会把太阳的暖色回射进反射。权重
            // 刻意低于遮挡的 0.85：净能量仍要随云量下降，回射只是把「蓝洞」染暖。
            let light = clamp(0.30 + 0.70 * max(dot(direction, to_sun), 0.0), 0.0, 1.0);
            let cloud_lit = mix(
                vec3<f32>(0.46, 0.50, 0.60),
                vec3<f32>(1.0, 0.94, 0.82),
                light,
            );
            color = color * (1.0 - density * 0.85) + cloud_lit * density * 0.14;
        }
    }
    // alpha 仍表示「几何命中权重」：天空回退不是命中，写 0。
    return vec4<f32>(color * reflection_weight(normal, view), 0.0);
}

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Out {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let clip = positions[index];
    var out: Out;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    // 这个 pass 的目标是半分辨率：`@builtin(position)` 是半分辨率像素坐标，拿深度图尺寸
    // （全分辨率）去除会把 UV 压到左上四分之一，整屏反射都在采样错的像素。UV 只能由顶点
    // 插值给出，与目标尺寸无关。
    out.uv = vec2<f32>(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5);
    return out;
}

@fragment
fn fs_main(input: Out) -> @location(0) vec4<f32> {
    let uv = input.uv;
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
                    return vec4<f32>(reflected_color * reflection_weight(normal, view), 1.0);
                }
            }
        }
    }
    return sky_reflection(normal, view, reflected, world);
}
