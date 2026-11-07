//! 会话级裁剪平面：CPU uniform 的布局固定为
//! `mat4 (64B) + position/time (16B) + clip (16B) + cloud (16B) = 112B`，
//! 着色器从第 80 字节读 clip、第 96 字节读云量。
//! 这里钉住数组形状与偏移；PBR 着色器的 naga 解析由 `pbr`/`shader_variants` 覆盖。

use super::*;

#[test]
fn camera_uniform_appends_the_clip_plane_at_byte_80() {
    let data = camera_uniform(
        &Mat4::identity(),
        Vec3::new(1.0, 2.0, 3.0),
        4.5,
        [0.0, 1.0, 0.0, -5.0],
        0.8,
        1.5,
        120.0,
        0.3,
        [0.9, 0.8, 0.7, 0.4],
        [0.7, 0.2],
        [0.25, 0.5],
        [0.1, 0.2, 0.3, 0.4],
    );
    assert_eq!(data.len(), 40, "160 bytes = 40 f32");
    assert_eq!(&data[16..20], &[1.0, 2.0, 3.0, 4.5]);
    assert_eq!(&data[20..24], &[0.0, 1.0, 0.0, -5.0]);
    assert_eq!(data[24], 0.8, "cloud coverage sits at byte 96");
    assert_eq!(data[25], 1.5, "cloud speed sits at byte 100");
    assert_eq!(data[26], 120.0, "cloud base sits at byte 104");
    assert_eq!(data[27], 0.3, "cloud shadow strength sits at byte 108");
    assert_eq!(&data[28..32], &[0.9, 0.8, 0.7, 0.4], "water tint sits at byte 112");
    assert_eq!(&data[32..34], &[0.7, 0.2], "surface cover sits at byte 128");
    assert_eq!(&data[34..36], &[0.25, 0.5], "ripple sits at byte 136");
    assert_eq!(&data[36..40], &[0.1, 0.2, 0.3, 0.4], "extra ripples sit at byte 144");

    let cleared = camera_uniform(
        &Mat4::identity(),
        Vec3::ZERO,
        0.0,
        [0.0; 4],
        0.5,
        1.0,
        100.0,
        0.26,
        [1.0, 1.0, 1.0, 0.0],
        [0.0, 0.0],
        [0.0, 1.5],
        [0.0, 1.5, 0.0, 1.5],
    );
    assert_eq!(&cleared[20..24], &[0.0; 4]);
    assert_eq!(cleared[24], 0.5);
    assert_eq!(cleared[25], 1.0);
    assert_eq!(cleared[26], 100.0);
    assert_eq!(cleared[27], 0.26);
    assert_eq!(&cleared[28..32], &[1.0, 1.0, 1.0, 0.0]);
    assert_eq!(&cleared[32..34], &[0.0, 0.0]);
}
