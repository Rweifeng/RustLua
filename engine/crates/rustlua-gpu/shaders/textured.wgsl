struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) sdf: u32,
};
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@vertex fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>, @location(3) sdf: u32) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    out.color = color;
    out.sdf = sdf;
    return out;
}
fn median(a: f32, b: f32, c: f32) -> f32 {
    return max(min(a, b), min(max(a, b), c));
}

fn msdf_alpha(sampled: vec4<f32>, screen_px_range: f32) -> f32 {
    let d = median(sampled.r, sampled.g, sampled.b);
    return clamp((d - 0.5) * screen_px_range + 0.5, 0.0, 1.0);
}

// 与 `vector_text.wgsl` 同一套：sRGB 目标在线性空间做 ALPHA_BLENDING，覆盖率要过一遍
// sRGB 传输曲线，混合结果才回到原生 UI 在 gamma 空间里的观感（否则浅色字边缘发虚）。
fn srgb_to_linear(c: f32) -> f32 {
    let v = clamp(c, 0.0, 1.0);
    return select(v / 12.92, pow((v + 0.055) / 1.055, 2.4), v > 0.04045);
}
fn gamma_coverage(coverage: f32, rgb: vec3<f32>) -> f32 {
    let a = clamp(coverage, 0.0, 1.0);
    let luma = clamp(dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
    // 与 `vector_text.wgsl` 同一套：按常见 HUD 对比度假设背景（亮字深底、暗字浅底），
    // 反解出线性混合要用的覆盖率，使字缘亮度与原生 gamma 空间混合一致。
    let bg_s = mix(0.72, 0.16, smoothstep(0.25, 0.75, luma));
    let bg_l = srgb_to_linear(bg_s);
    let want = srgb_to_linear(bg_s + (luma - bg_s) * a);
    return clamp((want - bg_l) / max(luma - bg_l, 1.0e-4), 0.0, 1.0);
}

@fragment fn fs_main(@location(0) uv: vec2<f32>, @location(1) color: vec4<f32>, @location(2) sdf: u32) -> @location(0) vec4<f32> {
    let sampled = textureSample(tex, samp, uv);
    if sdf == 1u {
        // MSDF 使用三个距离通道的中值（见 msdf_alpha），避免单通道切换造成笔画
        // 断裂和内部孔洞。
        let texture_size = vec2<f32>(textureDimensions(tex));
        // Distance range in atlas texels. `font.rs` rasterizes with
        // `Range::symmetric(PAD)` and msdfgen maps distance -> [0,1] as
        // `(distance/PAD + 1) * 0.5`, so the full [0,1] span covers
        // `2 * PAD` texels.
        //
        // TextMeshPro 对齐：`px_range = PAD`（而非 `2*PAD`）。在 1:1 渲染时
        // AA 渐变恰好覆盖 1 屏幕像素（锐利边缘）；小字号（如 14px）在
        // RASTER_SCALE=192 下 texels_per_pixel ≈ 4，screen_px_range ≈ 2，
        // AA 渐变 ~2px——可接受的清晰度。旧值 `2*PAD=16` 将渐变拉宽到 4px
        // 是小字号模糊的直接原因（以"填补窄笔画"为名的 workaround，实际效果
        // 是让所有尺寸的文字都变糊）。
        let px_range = 8.0;
        let texels_per_pixel = max(
            fwidth(uv.x) * texture_size.x,
            fwidth(uv.y) * texture_size.y,
        );
        let screen_px_range = max(
            px_range / max(texels_per_pixel, 1.0e-6),
            1.0,
        );

        // 每像素只取一个高分辨率图集采样点时，约 1.7px 宽的竖笔画（I、连字符）
        // 可能整条落在采样点之间而消失（WebGL2 小字号实测）。在像素内取 4 个
        // 象限角点做覆盖超采样：笔画至少被一个角点覆盖，平均后仍有 alpha。
        // 偏移量跟着 fwidth 缩放，大字号下退化为同一采样点，不会把大字糊掉。
        let half_px = vec2<f32>(fwidth(uv.x), fwidth(uv.y)) * 0.25;
        let a = 0.25 * (
            msdf_alpha(textureSampleLevel(tex, samp, uv + vec2<f32>(-half_px.x, -half_px.y), 0.0), screen_px_range)
            + msdf_alpha(textureSampleLevel(tex, samp, uv + vec2<f32>(half_px.x, -half_px.y), 0.0), screen_px_range)
            + msdf_alpha(textureSampleLevel(tex, samp, uv + vec2<f32>(-half_px.x, half_px.y), 0.0), screen_px_range)
            + msdf_alpha(textureSampleLevel(tex, samp, uv + vec2<f32>(half_px.x, half_px.y), 0.0), screen_px_range)
        );
        // 直通 alpha：管线 ALPHA_BLENDING 已按 alpha 预乘源色，不能再乘 a。
        return vec4<f32>(color.rgb, gamma_coverage(a, color.rgb));
    }
    if sdf == 2u {
        // Bitmap 路径：小字号下 Nearest 采样取 MSDF 中值，硬阈值化得到清晰字形边缘。
        // 避免 Linear 滤波在小尺寸下把距离场平均成模糊一片。
        let d = median(sampled.r, sampled.g, sampled.b);
        let a = select(0.0, 1.0, d >= 0.5);
        return vec4<f32>(color.rgb, a);
    }
    return sampled * color;
}
