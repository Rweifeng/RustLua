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

@fragment fn fs_main(@location(0) uv: vec2<f32>, @location(1) color: vec4<f32>, @location(2) sdf: u32) -> @location(0) vec4<f32> {
    let sampled = textureSample(tex, samp, uv);
    if sdf == 1u {
        // MSDF 使用三个距离通道的中值，避免单通道切换造成笔画断裂和内部孔洞。
        let d = median(sampled.r, sampled.g, sampled.b);
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

        let a = clamp(
            (d - 0.5) * screen_px_range + 0.5,
            0.0,
            1.0
        );
        // 直通 alpha：管线 ALPHA_BLENDING 已按 alpha 预乘源色，不能再乘 a。
        return vec4<f32>(color.rgb, a);
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
