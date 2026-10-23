//! P3-R02-1：PBR 的 CPU 参考侧——法线矩阵与 Cook-Torrance 直射项。
//!
//! 这一组不需要 GPU：先把"逆矩阵法线""GGX 的粗糙度行为""金属度吃掉漫反射""刚体旋转
//! 不变"这些性质在 CPU 上钉死，GPU golden（`wp0_pbr_golden`）才有可对照的期望。

use super::*;
use crate::host::renderer::pbr::{MIN_ROUGHNESS, env_brdf_approx, specular_lod};
use rustlua_assets::env_asset::sh_irradiance;
use rustlua_math::math3d::{Mat4, Quat, Vec3};
use rustlua_render::render_settings::{EnvironmentLighting, EnvironmentSettings, IblMode};

fn apply(matrix: Mat4, value: [f32; 3]) -> [f32; 3] {
    [
        matrix.columns[0][0] * value[0]
            + matrix.columns[1][0] * value[1]
            + matrix.columns[2][0] * value[2],
        matrix.columns[0][1] * value[0]
            + matrix.columns[1][1] * value[1]
            + matrix.columns[2][1] * value[2],
        matrix.columns[0][2] * value[0]
            + matrix.columns[1][2] * value[1]
            + matrix.columns[2][2] * value[2],
    ]
}

fn assert_close(actual: [f32; 3], expected: [f32; 3], epsilon: f32) {
    for channel in 0..3 {
        assert!(
            (actual[channel] - expected[channel]).abs() <= epsilon,
            "第 {channel} 分量不匹配：actual={actual:?} expected={expected:?}"
        );
    }
}

/// 非均匀缩放下必须走逆转置：直接把 `mat3(model)` 当法线变换会歪掉。
#[test]
fn normal_matrix_handles_non_uniform_scale() {
    let rotation = Mat4::from_quat(Quat::from_axis_angle(Vec3::new(0.0, 0.0, 1.0), 0.6).unwrap());
    let model = rotation * Mat4::scale(Vec3::new(2.0, 1.0, 0.5));
    let normal = normal_matrix(model);
    // 缩放 (2,1,0.5) 下，法线按 (1/2, 1, 2) 反向缩放，再叠加旋转。
    for (local, scaled) in [
        ([1.0, 0.0, 0.0], [0.5, 0.0, 0.0]),
        ([0.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, 1.0], [0.0, 0.0, 2.0]),
    ] {
        let expected = rotation
            .transform_point(Vec3::new(scaled[0], scaled[1], scaled[2]))
            .expect("刚性变换");
        let actual = apply(normal, local);
        // 两端都归一化后只比较方向。
        let expected = {
            let length =
                (expected.x * expected.x + expected.y * expected.y + expected.z * expected.z)
                    .sqrt();
            [
                expected.x / length,
                expected.y / length,
                expected.z / length,
            ]
        };
        let length = (actual[0] * actual[0] + actual[1] * actual[1] + actual[2] * actual[2]).sqrt();
        assert!(length > 0.0);
        assert_close(
            [actual[0] / length, actual[1] / length, actual[2] / length],
            expected,
            1e-5,
        );
    }
}

/// 纯旋转时法线矩阵就是旋转本身；平移不得混进法线。
#[test]
fn normal_matrix_is_rotation_for_rigid_transform() {
    let rotation = Mat4::from_quat(Quat::from_axis_angle(Vec3::new(0.3, 1.0, 0.2), 1.1).unwrap());
    let model = Mat4::translation(Vec3::new(3.0, -2.0, 5.0)) * rotation;
    let normal = normal_matrix(model);
    for local in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
        let expected = rotation
            .transform_point(Vec3::new(local[0], local[1], local[2]))
            .expect("刚性变换");
        assert_close(
            apply(normal, local),
            [expected.x, expected.y, expected.z],
            1e-5,
        );
    }
    assert_eq!(normal.columns[3], [0.0, 0.0, 0.0, 1.0]);
}

/// 奇异矩阵（scale=0）退回单位阵：宁可法线不对，也不让 NaN 传播到整帧。
#[test]
fn normal_matrix_falls_back_to_identity_on_singular_model() {
    let singular = Mat4::scale(Vec3::new(1.0, 0.0, 1.0));
    assert_eq!(normal_matrix(singular).columns, Mat4::identity().columns);
}

/// 5×5 金属度/粗糙度网格 × 多组方向：结果必须全程有限，且金属度=1 时漫反射为 0。
#[test]
fn brdf_is_finite_across_material_grid() {
    let normals = [
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.6, 0.8, 0.0],
        [0.0, -1.0, 0.0],
    ];
    let views = [[0.0, 0.0, 1.0], [0.5, 0.5, 0.7], [0.0, -1.0, 0.0]];
    let lights = [[0.3, -1.0, 0.2], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]];
    let mut checked = 0_u32;
    for metallic in [0.0, 0.25, 0.5, 0.75, 1.0] {
        for roughness in [MIN_ROUGHNESS, 0.2, 0.5, 0.8, 1.0] {
            for normal in normals {
                for view in views {
                    for light in lights {
                        let brdf = direct_brdf(
                            normal,
                            view,
                            light,
                            [0.42, 0.36, 0.28],
                            metallic,
                            roughness,
                        );
                        for value in brdf.diffuse.iter().chain(brdf.specular.iter()) {
                            assert!(
                                value.is_finite() && *value >= 0.0,
                                "BRDF 非法：metallic={metallic} roughness={roughness} \
                                 normal={normal:?} view={view:?} light={light:?} -> {value}"
                            );
                        }
                        if metallic == 1.0 {
                            assert_eq!(
                                brdf.diffuse, [0.0; 3],
                                "金属度=1 时不应有漫反射：roughness={roughness} normal={normal:?}"
                            );
                        }
                        checked += 1;
                    }
                }
            }
        }
    }
    assert_eq!(checked, 5 * 5 * 4 * 3 * 3);
}

/// 粗糙度越低，镜面峰越尖：正对反射方向时低粗糙度的镜面项必须显著更大。
#[test]
fn lower_roughness_sharpens_specular_peak() {
    let view = [0.0, 0.0, 1.0];
    let light = [0.0, 0.0, 1.0];
    let sharp = direct_brdf([0.0, 0.0, 1.0], view, light, [0.8, 0.8, 0.8], 0.0, 0.1);
    let rough = direct_brdf([0.0, 0.0, 1.0], view, light, [0.8, 0.8, 0.8], 0.0, 0.9);
    let sharp_specular: f32 = sharp.specular.iter().sum();
    let rough_specular: f32 = rough.specular.iter().sum();
    assert!(
        sharp_specular > rough_specular * 5.0,
        "低粗糙度镜面峰没有明显更尖：sharp={sharp_specular} rough={rough_specular}"
    );
}

/// 背光/零向量/非有限输入返回全零，而不是 NaN。
#[test]
fn brdf_rejects_degenerate_inputs() {
    let backlight = direct_brdf(
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
        [1.0, 1.0, 1.0],
        0.0,
        0.5,
    );
    assert_eq!(backlight, DirectBrdf::ZERO);
    assert_eq!(
        direct_brdf(
            [0.0; 3],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [1.0; 3],
            0.0,
            0.5
        ),
        DirectBrdf::ZERO
    );
    assert_eq!(
        direct_brdf(
            [f32::NAN, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [1.0; 3],
            0.0,
            0.5
        ),
        DirectBrdf::ZERO
    );
    assert_eq!(
        direct_brdf(
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [1.0; 3],
            f32::INFINITY,
            0.5
        ),
        DirectBrdf::ZERO
    );
}

/// 刚体旋转法线/视线/光线三者一起转，BRDF 数值不变（着色器里法线矩阵存在的意义）。
#[test]
fn brdf_is_rotation_invariant() {
    let rotation = Quat::from_axis_angle(Vec3::new(1.0, 0.3, 0.2), 0.9).expect("unit axis");
    let rotate = |value: [f32; 3]| {
        let rotated = rotation
            .rotate(Vec3::new(value[0], value[1], value[2]))
            .expect("rotate");
        [rotated.x, rotated.y, rotated.z]
    };
    let normal = [0.2, 1.0, 0.1];
    let view = [0.3, 0.4, 1.0];
    let light = [0.3, -1.0, 0.2];
    let reference = direct_brdf(normal, view, light, [0.5, 0.4, 0.3], 0.4, 0.35);
    let rotated = direct_brdf(
        rotate(normal),
        rotate(view),
        rotate(light),
        [0.5, 0.4, 0.3],
        0.4,
        0.35,
    );
    assert_close(rotated.total(), reference.total(), 1e-5);
}

/// P3-R03-2：SH9 的 CPU 参考与 uniform 布局。
///
/// 常量白环境（L=1）的 SH0 = 4π·Y00；因为系数已含 cosine 卷积，任意方向的
/// `sh_irradiance` 都必须回到 E/π = 1——着色器用同一组常数，后续 IBL golden 才有对照。
#[test]
fn constant_environment_sh_evaluates_to_unit_irradiance() {
    let mut sh = [0.0_f32; 27];
    let sh0 = 4.0 * std::f32::consts::PI * 0.282_095;
    sh[0] = sh0;
    sh[1] = sh0;
    sh[2] = sh0;
    let lighting = EnvironmentLighting {
        sh,
        settings: EnvironmentSettings {
            intensity: 2.5,
            rotation: 0.75,
        },
    };
    let uniform = lighting.uniform(IblMode::Diffuse);
    assert_eq!(&uniform[0..4], &[sh0, sh0, sh0, 0.0], "SH0 独占一个 vec4");
    assert!(
        uniform[4..36].iter().all(|value| *value == 0.0),
        "其余 SH 槽必须为零"
    );
    assert_eq!(&uniform[36..40], &[2.5, 0.75, 1.0, 0.0], "params 布局");
    let full = lighting.uniform(IblMode::Full);
    assert_eq!(full[38], 3.0);
    for normal in [
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(0.3, 0.5, -0.8),
    ] {
        let rgb = sh_irradiance(&sh, normal);
        for channel in rgb {
            assert!(
                (channel - 1.0).abs() < 1e-4,
                "常量环境辐照度必须为 1：normal={normal:?} rgb={rgb:?}"
            );
        }
    }
    // 默认构造写全零 SH + Off 模式码：未设置环境时 Off 分支不读，但残留值不能来自上一帧。
    let default_uniform = EnvironmentLighting::default().uniform(IblMode::Off);
    assert!(default_uniform[..36].iter().all(|value| *value == 0.0));
    assert_eq!(default_uniform[38], 0.0, "Off 模式码必须为 0");
}

/// P3-R03-3：`EnvBRDFApprox` 的 CPU 参考性质。
///
/// 关键锚点：`F0=1、roughness=0、N·V=1` 时权重应为 1（镜面全反射）；F0 越大权重越大；
/// 零/非有限输入不产生 NaN/Inf。GPU 侧用同一组常数，所以这些读数可以直接对照。
#[test]
fn env_brdf_approx_is_energy_bounded_and_monotonic_in_f0() {
    let mirror = env_brdf_approx([1.0; 3], 0.0, 1.0);
    for channel in mirror {
        assert!((channel - 1.0).abs() < 1e-3, "镜面锚点必须为 1：{mirror:?}");
    }
    let dielectric = env_brdf_approx([0.04; 3], 0.5, 0.7);
    let metal = env_brdf_approx([0.9; 3], 0.5, 0.7);
    for channel in 0..3 {
        assert!(dielectric[channel] >= 0.0 && dielectric[channel] <= 1.0);
        assert!(metal[channel] > dielectric[channel]);
    }
    assert_eq!(env_brdf_approx([f32::NAN, 0.0, 0.0], 0.5, 0.5), [0.0; 3]);
    assert_eq!(env_brdf_approx([0.5; 3], f32::INFINITY, 0.5), [0.0; 3]);
    assert_eq!(env_brdf_approx([0.5; 3], 0.5, f32::NAN), [0.0; 3]);
    // 极低 F0 不允许出现负权重（近似公式未夹时会给出 -0.04）。
    for roughness in [0.0, 0.25, 0.5, 0.75, 1.0] {
        for n_dot_v in [0.0, 0.3, 1.0] {
            for channel in env_brdf_approx([0.0; 3], roughness, n_dot_v) {
                assert!(channel >= 0.0, "负权重：r={roughness} ndv={n_dot_v}");
            }
        }
    }
}

/// P3-R03-3：mip 选择与烘焙约定互逆。
#[test]
fn specular_lod_maps_roughness_across_prefiltered_mips() {
    assert_eq!(specular_lod(0.0, 5), 0.0);
    assert_eq!(specular_lod(0.5, 5), 2.0);
    assert_eq!(specular_lod(1.0, 5), 4.0);
    assert_eq!(specular_lod(0.5, 1), 0.0, "单 mip 只能采第 0 级");
    assert_eq!(specular_lod(0.5, 0), 0.0, "无 mip 不越界");
    assert_eq!(specular_lod(-1.0, 5), 0.0, "下限夹紧");
    assert_eq!(specular_lod(2.0, 5), 4.0, "上限夹紧");
}
