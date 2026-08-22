//! Self-authored skinned + animated GLB for M8-07 tests and the 3D demo.
//!
//! A quad with two joints (left vertices bound to Joint0, right to Joint1) and
//! one animation channel that rotates Joint1 around +Y, so the right half
//! bends. Reproducible: run this example to regenerate
//! `game/assets/models/m8-skin-quad.glb`.

use std::fs;

const OUT_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../game/assets/models/m8-skin-quad.glb"
);

fn push_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn main() {
    // ---- BIN layout -------------------------------------------------------
    let mut bin = Vec::new();
    let positions_offset = 0usize;
    let positions = [
        [-0.5f32, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [-0.5, 0.5, 0.0],
        [0.5, 0.5, 0.0],
    ];
    for position in positions {
        for component in position {
            push_f32(&mut bin, component);
        }
    }
    let joints_offset = bin.len();
    let joints = [[0u16, 0, 0, 0], [0, 1, 0, 0], [0, 0, 0, 0], [0, 1, 0, 0]];
    for joint in joints {
        for component in joint {
            push_u16(&mut bin, component);
        }
    }
    let weights_offset = bin.len();
    let weights = [
        [1.0f32, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
    ];
    for weight in weights {
        for component in weight {
            push_f32(&mut bin, component);
        }
    }
    let indices_offset = bin.len();
    for index in [0u16, 1, 2, 2, 1, 3] {
        push_u16(&mut bin, index);
    }
    let inverse_bind_offset = bin.len();
    for _ in 0..2 {
        for index in 0..16 {
            push_f32(&mut bin, if index % 5 == 0 { 1.0 } else { 0.0 });
        }
    }
    let anim_times_offset = bin.len();
    for time in [0.0f32, 1.0] {
        push_f32(&mut bin, time);
    }
    let anim_rotation_offset = bin.len();
    let half_pi = std::f32::consts::FRAC_PI_2;
    let sin_half = half_pi * 0.5;
    // t=0 identity; t=1 rotate +90° around Y.
    for value in [
        0.0f32,
        0.0,
        0.0,
        1.0,
        0.0,
        sin_half.sin(),
        0.0,
        sin_half.cos(),
    ] {
        push_f32(&mut bin, value);
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }

    // ---- JSON -------------------------------------------------------------
    let json = format!(
        r#"{{"asset":{{"version":"2.0","generator":"p2 self-authored M8-07 skinned test asset"}},
"scene":0,"scenes":[{{"nodes":[0]}}],
"nodes":[
  {{"name":"Root","children":[1,2]}},
  {{"name":"Joint0"}},
  {{"name":"Joint1"}},
  {{"name":"SkinnedQuad","mesh":0,"skin":0}}
],
"skins":[{{"joints":[1,2],"inverseBindMatrices":4,"skeleton":1}}],
"animations":[{{"name":"Bend","channels":[{{"sampler":0,"target":{{"node":2,"path":"rotation"}}}}],"samplers":[{{"input":5,"output":6}}]}}],
"meshes":[{{"name":"SkinnedQuad","primitives":[{{"attributes":{{"POSITION":0,"JOINTS_0":1,"WEIGHTS_0":2}},"indices":3}}]}}],
"bufferViews":[
  {{"buffer":0,"byteOffset":{positions_offset},"byteLength":48,"target":34962}},
  {{"buffer":0,"byteOffset":{joints_offset},"byteLength":32,"target":34962}},
  {{"buffer":0,"byteOffset":{weights_offset},"byteLength":64,"target":34962}},
  {{"buffer":0,"byteOffset":{indices_offset},"byteLength":12,"target":34963}},
  {{"buffer":0,"byteOffset":{inverse_bind_offset},"byteLength":128}},
  {{"buffer":0,"byteOffset":{anim_times_offset},"byteLength":8}},
  {{"buffer":0,"byteOffset":{anim_rotation_offset},"byteLength":32}}
],
"accessors":[
  {{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[-0.5,-0.5,0.0],"max":[0.5,0.5,0.0]}},
  {{"bufferView":1,"componentType":5123,"count":4,"type":"VEC4"}},
  {{"bufferView":2,"componentType":5126,"count":4,"type":"VEC4"}},
  {{"bufferView":3,"componentType":5123,"count":6,"type":"SCALAR"}},
  {{"bufferView":4,"componentType":5126,"count":2,"type":"MAT4"}},
  {{"bufferView":5,"componentType":5126,"count":2,"type":"SCALAR"}},
  {{"bufferView":6,"componentType":5126,"count":2,"type":"VEC4"}}
],
"buffers":[{{"byteLength":{}}}]}}"#,
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
    println!("gen_m8_skin_glb_ok bytes={} path={OUT_PATH}", out.len());
}
