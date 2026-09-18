//! WGSL shader sources embedded at compile time (renderer-only).

pub const VECTOR_TEXT_SHADER: &str = r#"
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
"#;

pub const TEXTURED_SHADER: &str = r#"
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
        // `2 * PAD` texels. Using PAD here halves the range and doubles the
        // AA ramp width (2px instead of 1px), which merges thin strokes and
        // fills narrow counters at small sizes ("white blocks / dropped
        // strokes"). Keep this equal to `2 * font::PAD` (PAD = 8).
        let px_range = 16.0;
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
    return sampled * color;
}
"#;

/// WP6 世界 Sprite 通道：顶点是**世界坐标** `(x, y, z)`，投影由顶点着色器用 2D 相机的
/// `view_projection` 完成（`Camera2D::view_projection`，与 CPU 侧 `world_to_screen` 同源）。
/// 片元与 [`TEXTURED_SHADER`] 的普通纹理分支一致（采样 × 顶点色），因此世界路径与 overlay
/// 路径在颜色上不会分叉；两者的差异只有"投影在哪算"和"是否参与深度测试"。
pub const WORLD_SPRITE_SHADER: &str = r#"
struct Camera { view_projection: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};
@vertex fn vs_main(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = camera.view_projection * vec4<f32>(position, 1.0);
    out.uv = uv;
    out.color = color;
    return out;
}
@fragment fn fs_main(@location(0) uv: vec2<f32>, @location(1) color: vec4<f32>) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, uv) * color;
}
"#;

pub const MESH3D_SHADER: &str = r#"
struct Camera { view_projection: mat4x4<f32> };
struct Light { direction: vec4<f32>, color_intensity: vec4<f32> };
struct Material { base_color: vec4<f32>, surface: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> light: Light;
@group(0) @binding(2) var<uniform> shadow_camera: Camera;
@group(0) @binding(3) var shadow_map: texture_depth_2d;
@group(0) @binding(4) var shadow_sampler: sampler_comparison;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var<uniform> joint_palette: array<mat4x4<f32>, 128>;
@group(1) @binding(2) var<storage, read> instance_models: array<mat4x4<f32>>;
@group(1) @binding(3) var albedo: texture_2d<f32>;
@group(1) @binding(4) var albedo_sampler: sampler;
struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) normal: vec3<f32>, @location(1) shadow_position: vec4<f32>, @location(2) uv: vec2<f32> };
@vertex fn vs_main(@builtin(instance_index) instance_index: u32, @location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) joint_indices: vec4<u32>, @location(4) bone_weights: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
    let model = instance_models[instance_index];
    let skinned = material.surface.z > 0.5;
    var local_position = vec4<f32>(position, 1.0);
    var local_normal = vec4<f32>(normal, 0.0);
    if skinned {
        var skinned_position = vec3<f32>(0.0);
        var skinned_normal = vec3<f32>(0.0);
        skinned_position += (joint_palette[joint_indices.x] * local_position).xyz * bone_weights.x;
        skinned_position += (joint_palette[joint_indices.y] * local_position).xyz * bone_weights.y;
        skinned_position += (joint_palette[joint_indices.z] * local_position).xyz * bone_weights.z;
        skinned_position += (joint_palette[joint_indices.w] * local_position).xyz * bone_weights.w;
        skinned_normal += (joint_palette[joint_indices.x] * local_normal).xyz * bone_weights.x;
        skinned_normal += (joint_palette[joint_indices.y] * local_normal).xyz * bone_weights.y;
        skinned_normal += (joint_palette[joint_indices.z] * local_normal).xyz * bone_weights.z;
        skinned_normal += (joint_palette[joint_indices.w] * local_normal).xyz * bone_weights.w;
        local_position = vec4<f32>(skinned_position, 1.0);
        local_normal = vec4<f32>(skinned_normal, 0.0);
    }
    let world_position = model * local_position;
    out.position = camera.view_projection * world_position;
    out.normal = local_normal.xyz;
    out.shadow_position = shadow_camera.view_projection * world_position;
    out.uv = uv;
    return out;
}
@fragment fn fs_main(@location(0) normal: vec3<f32>, @location(1) shadow_position: vec4<f32>, @location(2) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let albedo_color = textureSample(albedo, albedo_sampler, uv);
    let base_rgb = material.base_color.rgb * albedo_color.rgb;
    let base_alpha = material.base_color.a * albedo_color.a;
    let n = normalize(normal);
    let l = normalize(-light.direction.xyz);
    let diffuse = max(dot(n, l), 0.0);
    let roughness = clamp(material.surface.y, 0.04, 1.0);
    let metallic = clamp(material.surface.x, 0.0, 1.0);
    let specular = pow(max(dot(reflect(-l, n), vec3<f32>(0.0, 0.0, 1.0)), 0.0), 2.0 / (roughness * roughness));
    let shadow_ndc = shadow_position.xyz / shadow_position.w;
    let shadow_uv = vec2<f32>(shadow_ndc.x * 0.5 + 0.5, 1.0 - (shadow_ndc.y * 0.5 + 0.5));
    let shadow_depth = shadow_ndc.z - 0.002;
    let shadow_visibility = select(1.0, textureSampleCompare(shadow_map, shadow_sampler, shadow_uv, shadow_depth), shadow_uv.x >= 0.0 && shadow_uv.x <= 1.0 && shadow_uv.y >= 0.0 && shadow_uv.y <= 1.0 && shadow_ndc.z >= 0.0 && shadow_ndc.z <= 1.0);
    let brightness = (0.15 + (diffuse * (1.0 - metallic) + specular * mix(0.04, 1.0, metallic)) * shadow_visibility) * light.color_intensity.w;
    return vec4<f32>(base_rgb * light.color_intensity.rgb * brightness, base_alpha);
}
"#;

pub const SHADOW_SHADER: &str = r#"
struct Camera { view_projection: mat4x4<f32> };
struct Material { base_color: vec4<f32>, surface: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var<uniform> joint_palette: array<mat4x4<f32>, 128>;
@group(1) @binding(2) var<storage, read> instance_models: array<mat4x4<f32>>;
@vertex fn vs_main(@builtin(instance_index) instance_index: u32, @location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) joint_indices: vec4<u32>, @location(4) bone_weights: vec4<f32>) -> @builtin(position) vec4<f32> {
    let model = instance_models[instance_index];
    let skinned = material.surface.z > 0.5;
    var local_position = vec4<f32>(position, 1.0);
    if skinned {
        var skinned_position = vec3<f32>(0.0);
        skinned_position += (joint_palette[joint_indices.x] * local_position).xyz * bone_weights.x;
        skinned_position += (joint_palette[joint_indices.y] * local_position).xyz * bone_weights.y;
        skinned_position += (joint_palette[joint_indices.z] * local_position).xyz * bone_weights.z;
        skinned_position += (joint_palette[joint_indices.w] * local_position).xyz * bone_weights.w;
        local_position = vec4<f32>(skinned_position, 1.0);
    }
    return camera.view_projection * model * local_position;
}
"#;
