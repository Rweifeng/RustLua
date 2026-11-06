//! 会话级裁剪平面：CPU uniform 的布局固定为
//! `mat4 (64B) + position/time (16B) + clip (16B) = 96B`，着色器从第 80 字节读 clip。
//! 这里钉住数组形状与偏移；PBR 着色器的 naga 解析由 `pbr`/`shader_variants` 覆盖。

use super::*;

#[test]
fn camera_uniform_appends_the_clip_plane_at_byte_80() {
    let data = camera_uniform(
        &Mat4::identity(),
        Vec3::new(1.0, 2.0, 3.0),
        4.5,
        [0.0, 1.0, 0.0, -5.0],
    );
    assert_eq!(data.len(), 24, "96 bytes = 24 f32");
    assert_eq!(&data[16..20], &[1.0, 2.0, 3.0, 4.5]);
    assert_eq!(&data[20..24], &[0.0, 1.0, 0.0, -5.0]);

    let cleared = camera_uniform(&Mat4::identity(), Vec3::ZERO, 0.0, [0.0; 4]);
    assert_eq!(&cleared[20..24], &[0.0; 4]);
}
