//! P3-R02-1：PBR 直射光的 **CPU 参考实现**与逐实例法线矩阵。
//!
//! 这个文件不是"第二套渲染路径"，而是验收仪器：CPU 侧用与 `shaders/mesh3d_pbr.wgsl`
//! 逐行对应的公式算出一个表面点的颜色，再与 GPU 读回对拍。没有它，"金属度/粗糙度网格
//! ±2/255"只能靠肉眼看图，错了也说不清是哪一项。
//!
//! 约定：
//! - 所有方向向量输入都按"从表面指出"理解（`light` 是表面→光源），函数内部会归一化；
//! - [`DirectBrdf::diffuse`] / [`DirectBrdf::specular`] 已经乘过 `N·L`，与着色器里的
//!   `direct` 同口径；辐射亮度、阴影可见性、环境项由调用方再乘；
//! - 非法输入（非有限、零长度、背光）返回全零，不产生 NaN/Inf。

use rustlua_math::math3d::Mat4;

pub const PI: f32 = std::f32::consts::PI;
pub const DIELECTRIC_F0: f32 = 0.04;
pub const AMBIENT_FACTOR: f32 = 0.15;
pub const MIN_ROUGHNESS: f32 = 0.045;

/// 一个表面点的直射光 BRDF 结果（漫反射 + 镜面，已乘 `N·L`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirectBrdf {
    pub diffuse: [f32; 3],
    pub specular: [f32; 3],
}

impl DirectBrdf {
    pub const ZERO: Self = Self {
        diffuse: [0.0; 3],
        specular: [0.0; 3],
    };

    #[must_use]
    pub fn total(&self) -> [f32; 3] {
        [
            self.diffuse[0] + self.specular[0],
            self.diffuse[1] + self.specular[1],
            self.diffuse[2] + self.specular[2],
        ]
    }
}

/// 逐实例法线矩阵：`transpose(inverse(mat3(model)))`，平移分量清零。
///
/// 奇异矩阵（某个轴 scale=0）返回单位阵：调用方会记一次告警，但**不会**让整帧的着色器
/// 读到 NaN。非均匀缩放下必须走逆矩阵，不能只乘 `mat3(model)`。
#[must_use]
pub fn normal_matrix(model: Mat4) -> Mat4 {
    let Ok(inverse) = model.inverse() else {
        return Mat4::identity();
    };
    Mat4 {
        columns: [
            [
                inverse.columns[0][0],
                inverse.columns[1][0],
                inverse.columns[2][0],
                0.0,
            ],
            [
                inverse.columns[0][1],
                inverse.columns[1][1],
                inverse.columns[2][1],
                0.0,
            ],
            [
                inverse.columns[0][2],
                inverse.columns[1][2],
                inverse.columns[2][2],
                0.0,
            ],
            [0.0, 0.0, 0.0, 1.0],
        ],
    }
}

/// Cook-Torrance 直射项（NDF: Trowbridge-Reitz / GGX；可见性: Smith height-correlated；
/// Fresnel: Schlick）。
#[must_use]
pub fn direct_brdf(
    normal: [f32; 3],
    view: [f32; 3],
    light: [f32; 3],
    base_color: [f32; 3],
    metallic: f32,
    roughness: f32,
) -> DirectBrdf {
    if !normal
        .iter()
        .chain(view.iter())
        .chain(light.iter())
        .chain(base_color.iter())
        .all(|value| value.is_finite())
        || !metallic.is_finite()
        || !roughness.is_finite()
    {
        return DirectBrdf::ZERO;
    }
    let Some(n) = normalize(normal) else {
        return DirectBrdf::ZERO;
    };
    let Some(v) = normalize(view) else {
        return DirectBrdf::ZERO;
    };
    let Some(l) = normalize(light) else {
        return DirectBrdf::ZERO;
    };
    let half = [l[0] + v[0], l[1] + v[1], l[2] + v[2]];
    let Some(h) = normalize(half) else {
        return DirectBrdf::ZERO;
    };
    let n_dot_l = dot(n, l).max(0.0);
    let n_dot_v = dot(n, v).max(0.0);
    if n_dot_l <= 0.0 || n_dot_v <= 0.0 {
        return DirectBrdf::ZERO;
    }
    let n_dot_h = dot(n, h).max(0.0);
    let v_dot_h = dot(v, h).max(0.0);
    let metallic = metallic.clamp(0.0, 1.0);
    let roughness = roughness.clamp(MIN_ROUGHNESS, 1.0);
    let alpha = roughness * roughness;
    let a2 = alpha * alpha;

    let d_denominator = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    let distribution = a2 / (PI * d_denominator * d_denominator).max(1e-7);
    let gv = n_dot_l * (n_dot_v * n_dot_v * (1.0 - a2) + a2).sqrt();
    let gl = n_dot_v * (n_dot_l * n_dot_l * (1.0 - a2) + a2).sqrt();
    let visibility = 0.5 / (gv + gl).max(1e-7);

    let fresnel_power = (1.0 - v_dot_h).clamp(0.0, 1.0).powi(5);
    let mut diffuse = [0.0; 3];
    let mut specular = [0.0; 3];
    for channel in 0..3 {
        let f0 = DIELECTRIC_F0 + (base_color[channel] - DIELECTRIC_F0) * metallic;
        let fresnel = f0 + (1.0 - f0) * fresnel_power;
        let kd = (1.0 - fresnel) * (1.0 - metallic);
        diffuse[channel] = kd * base_color[channel] / PI * n_dot_l;
        specular[channel] = distribution * visibility * fresnel * n_dot_l;
    }
    if diffuse
        .iter()
        .chain(specular.iter())
        .all(|value| value.is_finite())
    {
        DirectBrdf { diffuse, specular }
    } else {
        DirectBrdf::ZERO
    }
}

/// Karis `EnvBRDFApprox` 的 CPU 参考（与 `mesh3d_pbr.wgsl::env_brdf_approx` 同式）。
///
/// 返回预过滤环境镜面项的权重；`f0` 逐通道、`roughness`/`n_dot_v` 在 `[0, 1]` 内。
/// 近似在极低 F0 时可能给出负值，这里与着色器一样夹到 0。
#[must_use]
pub fn env_brdf_approx(f0: [f32; 3], roughness: f32, n_dot_v: f32) -> [f32; 3] {
    if !f0.iter().all(|value| value.is_finite()) || !roughness.is_finite() || !n_dot_v.is_finite() {
        return [0.0; 3];
    }
    let roughness = roughness.clamp(0.0, 1.0);
    let n_dot_v = n_dot_v.clamp(0.0, 1.0);
    let r = [
        1.0 - roughness,
        0.0425 - roughness * 0.0275,
        1.04 - roughness * 0.572,
        roughness * 0.022 - 0.04,
    ];
    let a004 = (r[0] * r[0]).min((-9.28 * n_dot_v).exp2()) * r[0] + r[1];
    let env_x = -1.04 * a004 + r[2];
    let env_y = 1.04 * a004 + r[3];
    let mut result = [0.0; 3];
    for channel in 0..3 {
        result[channel] = (f0[channel] * env_x + env_y).max(0.0);
    }
    result
}

/// 镜面 IBL 的 mip 选择：`roughness * (mip_count - 1)`，与烘焙期的
/// `roughness = mip / (mip_count - 1)` 互逆；`mip_count = 0` 时返回 0。
#[must_use]
pub fn specular_lod(roughness: f32, mip_count: u32) -> f32 {
    roughness.clamp(0.0, 1.0) * mip_count.saturating_sub(1) as f32
}

fn normalize(value: [f32; 3]) -> Option<[f32; 3]> {
    let length_sq = dot(value, value);
    if !length_sq.is_finite() || length_sq <= 1e-12 {
        return None;
    }
    let inverse = length_sq.sqrt().recip();
    let result = [value[0] * inverse, value[1] * inverse, value[2] * inverse];
    result
        .iter()
        .all(|value| value.is_finite())
        .then_some(result)
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}
