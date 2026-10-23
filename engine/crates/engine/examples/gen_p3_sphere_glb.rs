//! Self-authored unit sphere GLB for `wp0_ibl_golden` (P3-R03-4).
//!
//! Reproducible: run this example to regenerate
//! `examples/game_api/assets/models/p3-sphere.glb`. The sphere is a standard
//! UV parameterisation (radius 1, 48×24), smooth normals, with seam/pole
//! vertices duplicated so UVs stay continuous.
//!
//! ```text
//! cargo run -p engine --example gen_p3_sphere_glb
//! ```

use std::fs;

const SEGMENTS: usize = 48;
const RINGS: usize = 24;
const OUT_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../examples/game_api/assets/models/p3-sphere.glb"
);

fn push_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn main() {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    for ring in 0..=RINGS {
        let theta = std::f32::consts::PI * ring as f32 / RINGS as f32;
        let (sin_theta, cos_theta) = theta.sin_cos();
        for segment in 0..=SEGMENTS {
            let phi = std::f32::consts::TAU * segment as f32 / SEGMENTS as f32;
            let (sin_phi, cos_phi) = phi.sin_cos();
            let position = [sin_theta * cos_phi, cos_theta, sin_theta * sin_phi];
            positions.extend_from_slice(&position);
            normals.extend_from_slice(&position);
            uvs.push(segment as f32 / SEGMENTS as f32);
            uvs.push(ring as f32 / RINGS as f32);
        }
    }
    let mut indices = Vec::new();
    let stride = SEGMENTS + 1;
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = (ring * stride + segment) as u16;
            let b = a + 1;
            let c = a + stride as u16;
            let d = c + 1;
            // 外侧看逆时针：cross(B-A, C-A) 指向球外。
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }

    let mut bin = Vec::new();
    let positions_offset = bin.len();
    for value in &positions {
        push_f32(&mut bin, *value);
    }
    let normals_offset = bin.len();
    for value in &normals {
        push_f32(&mut bin, *value);
    }
    let uvs_offset = bin.len();
    for value in &uvs {
        push_f32(&mut bin, *value);
    }
    let indices_offset = bin.len();
    for index in &indices {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }

    let vertex_count = positions.len() / 3;
    let json = format!(
        r#"{{"asset":{{"version":"2.0","generator":"RustLua self-authored P3 unit sphere"}},
"scene":0,"scenes":[{{"nodes":[0]}}],
"nodes":[{{"name":"UnitSphere","mesh":0}}],
"meshes":[{{"name":"UnitSphere","primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2}},"indices":3,"mode":4}}]}}],
"bufferViews":[
  {{"buffer":0,"byteOffset":{positions_offset},"byteLength":{},"target":34962}},
  {{"buffer":0,"byteOffset":{normals_offset},"byteLength":{},"target":34962}},
  {{"buffer":0,"byteOffset":{uvs_offset},"byteLength":{},"target":34962}},
  {{"buffer":0,"byteOffset":{indices_offset},"byteLength":{},"target":34963}}
],
"accessors":[
  {{"bufferView":0,"componentType":5126,"count":{vertex_count},"type":"VEC3","min":[-1.0,-1.0,-1.0],"max":[1.0,1.0,1.0]}},
  {{"bufferView":1,"componentType":5126,"count":{vertex_count},"type":"VEC3"}},
  {{"bufferView":2,"componentType":5126,"count":{vertex_count},"type":"VEC2"}},
  {{"bufferView":3,"componentType":5123,"count":{},"type":"SCALAR"}}
],
"buffers":[{{"byteLength":{}}}]}}"#,
        positions.len() * 4,
        normals.len() * 4,
        uvs.len() * 4,
        indices.len() * 2,
        indices.len(),
        bin.len()
    );
    let mut json_bytes = json.into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }

    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x4654_6C67_u32.to_le_bytes()); // glTF magic
    out.extend_from_slice(&2_u32.to_le_bytes()); // version
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534A_u32.to_le_bytes()); // JSON chunk
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942_u32.to_le_bytes()); // BIN chunk
    out.extend_from_slice(&bin);

    if let Some(parent) = std::path::Path::new(OUT_PATH).parent() {
        fs::create_dir_all(parent).expect("create output directory");
    }
    fs::write(OUT_PATH, &out).expect("write glb");
    println!(
        "gen_p3_sphere_glb_ok bytes={} vertices={vertex_count} triangles={} path={OUT_PATH}",
        out.len(),
        indices.len() / 3
    );
}
