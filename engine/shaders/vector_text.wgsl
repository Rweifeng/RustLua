struct Glyph { segment_start: u32, segment_count: u32, _padding: vec2<u32>, bounds: vec4<f32> };
struct Segment { start: vec2<f32>, end: vec2<f32>, bounds: vec4<f32> };
@group(0) @binding(0) var<storage, read> glyphs: array<Glyph>;
@group(0) @binding(1) var<storage, read> segments: array<Segment>;
struct Out { @builtin(position) position: vec4<f32>, @location(0) local: vec2<f32>, @location(1) color: vec4<f32>, @location(2) glyph: u32 };
@vertex fn vs_main(@location(0) position: vec2<f32>, @location(1) local: vec2<f32>, @location(2) glyph: u32, @location(3) color: vec4<f32>) -> Out { var o: Out; o.position=vec4<f32>(position,0.0,1.0); o.local=local; o.glyph=glyph; o.color=color; return o; }
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
  let aa=max(fwidth(local.x), fwidth(local.y)) + 0.002;
  let margin=aa;
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
      if distance <= aa && dot(d,d) > 0.0 {
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
  let coverage=smoothstep(-aa, aa, signed_distance);
  return vec4<f32>(color.rgb, coverage*color.a);
}
