struct Glyph { segment_start: u32, segment_count: u32, _padding: vec2<u32>, bounds: vec4<f32> };
struct Segment { start: vec2<f32>, end: vec2<f32>, bounds: vec4<f32> };
@group(0) @binding(0) var<storage, read> glyphs: array<Glyph>;
@group(0) @binding(1) var<storage, read> segments: array<Segment>;
struct Out { @builtin(position) position: vec4<f32>, @location(0) local: vec2<f32>, @location(1) color: vec4<f32>, @location(2) glyph: u32 };
// 小字号笔画膨胀量（物理像素/边），见 `fs_main` 里的注释。
const STEM_DARKEN: f32 = 0.25;
@vertex fn vs_main(@location(0) position: vec2<f32>, @location(1) local: vec2<f32>, @location(2) glyph: u32, @location(3) color: vec4<f32>) -> Out { var o: Out; o.position=vec4<f32>(position,0.0,1.0); o.local=local; o.glyph=glyph; o.color=color; return o; }
// 原生 UI（DirectWrite/GDI）在 gamma 空间做覆盖率混合；本工程渲染目标是 sRGB 格式，
// 硬件在**线性**空间做 ALPHA_BLENDING，浅色字的抗锯齿边缘会被抬亮成"发虚"的光晕。
// 把覆盖率先过一遍 sRGB 传输曲线，线性混合后的结果就回到 gamma 空间的观感：
// 浅色字用线性化后的覆盖率（边缘压暗），深色字用反曲线（边缘提亮）。
fn srgb_to_linear(c: f32) -> f32 {
  let v = clamp(c, 0.0, 1.0);
  return select(v / 12.92, pow((v + 0.055) / 1.055, 2.4), v > 0.04045);
}
fn gamma_coverage(coverage: f32, rgb: vec3<f32>) -> f32 {
  let a = clamp(coverage, 0.0, 1.0);
  let luma = clamp(dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
  // 原生混合的目标是 `bg + (fg-bg)*a` 的 **sRGB 编码值**；线性混合只能命中它的
  // 线性解码值。背景不可见，按常见 HUD 对比度取样：亮字假设深底、暗字假设浅底。
  let bg_s = mix(0.72, 0.16, smoothstep(0.25, 0.75, luma));
  let bg_l = srgb_to_linear(bg_s);
  let want = srgb_to_linear(bg_s + (luma - bg_s) * a);
  return clamp((want - bg_l) / max(luma - bg_l, 1.0e-4), 0.0, 1.0);
}
fn winding_at(point: vec2<f32>, segment_start: u32, segment_count: u32) -> i32 {
  var winding=0i;
  for (var i=0u; i<segment_count; i++) {
    let s=segments[segment_start+i];
    let crosses=(s.start.y <= point.y && point.y < s.end.y) || (s.end.y <= point.y && point.y < s.start.y);
    if crosses {
      let x=s.start.x+(point.y-s.start.y)*(s.end.x-s.start.x)/(s.end.y-s.start.y);
      if x > point.x { winding += select(-1i, 1i, s.end.y > s.start.y); }
    }
  }
  return winding;
}
@fragment fn fs_main(@location(0) local: vec2<f32>, @location(1) color: vec4<f32>, @location(2) glyph: u32) -> @location(0) vec4<f32> {
  let g=glyphs[glyph];
  // `local` 是 em 单位；fwidth(local) 就是**一物理像素**在 em 空间的长度。
  // 覆盖率用 1 物理像素宽的线性斜坡（像素中心 ±0.5px），与原生光栅器的面积覆盖
  // 同一口径。旧版 smoothstep(-aa, aa) 跨 2 物理像素，13px 字号下笔画本身才 1px，
  // 边缘占了字高的一成多——这是"字糊"的直接来源。
  let px=max(max(fwidth(local.x), fwidth(local.y)), 1.0e-5);
  let margin=px*(1.0+STEM_DARKEN);
  if local.x < g.bounds.x - margin || local.y < g.bounds.y - margin || local.x > g.bounds.z + margin || local.y > g.bounds.w + margin { discard; }
  let winding=winding_at(local,g.segment_start,g.segment_count);
  var nearest=1e9;
  for (var i=0u; i<g.segment_count; i++) {
    let s=segments[g.segment_start+i];
    let d=s.end-s.start;
    let q=local-s.start;
    let in_bounds=!(local.x < s.bounds.x - margin || local.y < s.bounds.y - margin || local.x > s.bounds.z + margin || local.y > s.bounds.w + margin);
    if in_bounds {
      let t=clamp(dot(q,d)/max(dot(d,d),1e-8),0.0,1.0);
      let distance=length(q-d*t);
      if distance <= px && dot(d,d) > 0.0 {
        let closest=s.start+d*t;
        let normal=normalize(vec2<f32>(-d.y,d.x));
        let inside_a=winding_at(closest+normal*0.001,g.segment_start,g.segment_count) != 0;
        let inside_b=winding_at(closest-normal*0.001,g.segment_start,g.segment_count) != 0;
        if inside_a != inside_b {
          nearest=min(nearest,distance);
        }
      }
    }
  }
  let signed_distance=select(-nearest, nearest, winding != 0i);
  // 未 hinting 的轮廓在 11px 字号下笔画只有 1~1.2 物理像素，常横跨两列各分到一半
  // 覆盖率，gamma 混合后整根笔画发灰；原生（GDI/DirectWrite）会把笔画吸附到整像素，
  // 所以看着更实。按 `STEM_DARKEN` 个物理像素做膨胀把重量补回来——42px 的标题上
  // 这点膨胀不到一个像素的一半，字号越大影响越小。
  let coverage=clamp(signed_distance/px + 0.5 + STEM_DARKEN, 0.0, 1.0);
  return vec4<f32>(color.rgb, gamma_coverage(coverage, color.rgb)*color.a);
}
