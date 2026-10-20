//! 空间化：听者位姿 → 「增益 + 声像」的**唯一来源**（M74-01，`tasks/plan/M74-01-3D空间音频.md`）。
//!
//! ## 为什么空间模型必须住在引擎里
//!
//! 两个平台的播放能力差得很远：桌面的 rodio 能做等功率声像，web/微信的
//! `HTMLAudioElement` / `InnerAudioContext` **只有音量**。如果"听感"由平台各自决定，同一份脚本
//! 在两端就是两套行为，而症状（"网页上左边那个声音也在正中"）不会指向任何一行代码。
//!
//! 所以这里定的是**模型权威、平台尽力**：本模块是纯函数（不碰设备、不碰 rodio、不碰时间），
//! 算出 [`SpatialParams`]；平台只负责"把这两个数应用出去"，做不到的那一端必须**显式降级**
//! （见 `sink::SpatialEmission` 的文档与 [`crate::audio::AudioSystem::play_at_on`]）。
//!
//! ## 距离模型为什么不用反平方
//!
//! 2D 玩法里"隔一堵墙就听不见"不是想要的效果，而反平方的听感拐点又几乎全落在近距离，
//! 调参只能靠耳朵试。这里取**线性 + 上下限**：`min_distance` 内不衰减，`[min, max]` 线性到 0。
//! 三个数都能被单测钉死，策划也能用语言描述（"30 米外听不见"）。

use thiserror::Error;

/// 距离衰减模型（米）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpatialModel {
    /// 这个距离以内**不**衰减（声音的"实体尺寸"）。
    pub min_distance: f32,
    /// 这个距离以外完全听不见；必须大于 [`Self::min_distance`]。
    pub max_distance: f32,
}

impl SpatialModel {
    /// 默认：1 m 内不衰减，30 m 外静音。
    pub const DEFAULT: Self = Self {
        min_distance: 1.0,
        max_distance: 30.0,
    };
}

impl Default for SpatialModel {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 听者位姿：位置 + 朝向基。
///
/// `forward` 指向听者正前方，`up` 只用来定右轴（`right = normalize(forward × up)`），
/// 与渲染相机的约定一致（右手系、`+Y` 为上）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    /// 两耳中点（世界坐标，米）。
    pub position: [f32; 3],
    /// 朝向（不必归一化）。
    pub forward: [f32; 3],
    /// 上方向（不必归一化；不能与 `forward` 平行）。
    pub up: [f32; 3],
}

impl Listener {
    /// 用显式位姿构造。
    #[must_use]
    pub const fn new(position: [f32; 3], forward: [f32; 3], up: [f32; 3]) -> Self {
        Self {
            position,
            forward,
            up,
        }
    }

    /// 从"位置 + 看向的目标点"构造（相机只有 `position`/`target` 时用这个），上方向取 `+Y`。
    #[must_use]
    pub fn facing(position: [f32; 3], target: [f32; 3]) -> Self {
        Self {
            position,
            forward: sub(target, position),
            up: [0.0, 1.0, 0.0],
        }
    }

    /// 默认听者：原点、朝 `-Z`、上 `+Y`（与相机的默认朝向一致）。
    #[must_use]
    pub const fn at_origin() -> Self {
        Self {
            position: [0.0; 3],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
        }
    }
}

/// 一次播放的空间参数：`gain ∈ [0, 1]` 与 `pan ∈ [-1, 1]`（`-1` 全左、`+1` 全右）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpatialParams {
    /// 距离衰减后的增益（与总线音量相乘）。
    pub gain: f32,
    /// 声像：听者右轴上的投影。
    pub pan: f32,
}

/// 空间参数的拒绝原因。
///
/// 与 `AudioError` 的其余变体同口径：**非法参数当场报错**，不静默夹紧——"我设了参数但声音没变"
/// 是最难查的一类问题。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum SpatialError {
    /// 输入里有 NaN / ±inf（上游算崩了，不该让它变成音量）。
    #[error("spatial: {0} is not finite")]
    NonFinite(&'static str),
    /// 模型本身不合法（`min < 0` 或 `max <= min`）。
    #[error("spatial: invalid model: {0}")]
    InvalidModel(&'static str),
    /// 听者的朝向基退化（`forward`/`up` 为零向量或两者平行），右轴不存在。
    #[error("spatial: listener basis is degenerate: {0}")]
    DegenerateBasis(&'static str),
    /// 还没接线：宿主没有把相机位姿灌进来，`play_at` 拒绝而不是按原点瞎衰减。
    #[error("spatial: no listener configured (host must call set_listener each frame)")]
    NoListener,
}

/// 算出一次播放的空间参数。
///
/// 纯函数：没有设备、没有时间、没有全局状态，因此同一个输入永远得到同一对数字。
/// 非法输入返回 [`SpatialError`]（不夹紧）。
pub fn evaluate(
    emitter: [f32; 3],
    listener: &Listener,
    model: SpatialModel,
) -> Result<SpatialParams, SpatialError> {
    for (name, value) in [
        ("emitter.x", emitter[0]),
        ("emitter.y", emitter[1]),
        ("emitter.z", emitter[2]),
        ("listener.position.x", listener.position[0]),
        ("listener.position.y", listener.position[1]),
        ("listener.position.z", listener.position[2]),
        ("listener.forward.x", listener.forward[0]),
        ("listener.forward.y", listener.forward[1]),
        ("listener.forward.z", listener.forward[2]),
        ("listener.up.x", listener.up[0]),
        ("listener.up.y", listener.up[1]),
        ("listener.up.z", listener.up[2]),
        ("model.min_distance", model.min_distance),
        ("model.max_distance", model.max_distance),
    ] {
        if !value.is_finite() {
            return Err(SpatialError::NonFinite(name));
        }
    }
    if model.min_distance < 0.0 {
        return Err(SpatialError::InvalidModel("min_distance must be >= 0"));
    }
    if model.max_distance <= model.min_distance {
        return Err(SpatialError::InvalidModel(
            "max_distance must be greater than min_distance",
        ));
    }

    let forward = normalize(listener.forward, "forward")?;
    let up = normalize(listener.up, "up")?;
    let right = normalize(
        cross(forward, up),
        "forward × up (forward is parallel to up)",
    )?;

    let delta = sub(emitter, listener.position);
    let distance = length(delta);
    let gain = if distance <= model.min_distance {
        1.0
    } else if distance >= model.max_distance {
        0.0
    } else {
        (model.max_distance - distance) / (model.max_distance - model.min_distance)
    };
    // 与听者同点：方向无定义，声像取中（而不是 0/0 变成 NaN）。
    let pan = if distance < 1e-6 {
        0.0
    } else {
        dot(scale(delta, 1.0 / distance), right).clamp(-1.0, 1.0)
    };
    Ok(SpatialParams { gain, pan })
}

/// 把 `pan` 折回一个**固定距离**上的位置（桌面侧喂给 `rodio::SpatialSink` 用）。
///
/// 为什么不是直接给 rodio 真实坐标：那等于让 rodio 用自己的距离模型、web 用我们的，
/// 两端听感分叉。为什么折回后不会二次衰减：rodio 的 `Spatial` 用 `1/dist²` 且 `.min(1.0)`，
/// 折回位置到两耳的距离恒在 [`FOLD_DISTANCE`] 附近（< 1 m）⇒ 距离因子恒为 1，
/// 真正生效的只有我们算的 `gain` 与由位置差产生的声像。
///
/// 返回值：`(左耳, 右耳, 折回后的声源位置)`。
#[must_use]
pub fn fold_to_ears(listener: &Listener, pan: f32) -> ([f32; 3], [f32; 3], [f32; 3]) {
    // 退化基在 `evaluate` 里已被拒绝；这里兜底成"正前方"，不让播放路径因为除零 panic。
    let forward = normalize(listener.forward, "forward").unwrap_or([0.0, 0.0, -1.0]);
    let up = normalize(listener.up, "up").unwrap_or([0.0, 1.0, 0.0]);
    let right = normalize(cross(forward, up), "right").unwrap_or([1.0, 0.0, 0.0]);
    (
        add(listener.position, scale(right, -EAR_OFFSET)),
        add(listener.position, scale(right, EAR_OFFSET)),
        add(
            listener.position,
            add(
                scale(right, pan * FOLD_DISTANCE),
                scale(forward, FOLD_DISTANCE),
            ),
        ),
    )
}

/// 两点距离（米）。空间相关的读数与断言都该用它，别在调用点各写一遍开方。
#[must_use]
pub fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    length(sub(a, b))
}

/// 折回位置到听者中点的距离（米）。取在 rodio 的"1 m 内不衰减"以内。
pub const FOLD_DISTANCE: f32 = 0.5;
/// 单耳偏离中点的距离（米）。两耳间距 0.2 m，取人耳尺度的量级。
pub const EAR_OFFSET: f32 = 0.1;

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

fn normalize(a: [f32; 3], what: &'static str) -> Result<[f32; 3], SpatialError> {
    let len = length(a);
    // 先比长度、再查有限性：NaN 会漏过任何"< 阈值"的判断，所以两个条件都得写出来
    // （写成 `!(len > eps)` 读着绕，也会被 clippy 拦）。
    if len < 1e-6 || !len.is_finite() {
        return Err(SpatialError::DegenerateBasis(what));
    }
    Ok(scale(a, 1.0 / len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listener() -> Listener {
        // 原点、朝 -Z、上 +Y ⇒ 右轴 = normalize((-Z) × (+Y)) = (+1, 0, 0)。
        Listener::at_origin()
    }

    #[test]
    fn same_point_is_full_gain_and_centered() {
        let params = evaluate([0.0, 0.0, 0.0], &listener(), SpatialModel::DEFAULT).unwrap();
        assert_eq!(params.gain, 1.0);
        assert_eq!(params.pan, 0.0);
    }

    #[test]
    fn straight_ahead_is_centered_and_attenuated_by_distance() {
        // 正前方 = -Z；右轴是 +X，所以正前方的投影为 0。
        let model = SpatialModel::DEFAULT;
        let params = evaluate([0.0, 0.0, -10.0], &listener(), model).unwrap();
        assert_eq!(params.pan, 0.0);
        // (30 - 10) / (30 - 1) = 20/29
        assert!(
            (params.gain - 20.0 / 29.0).abs() < 1e-6,
            "gain={}",
            params.gain
        );
    }

    #[test]
    fn hard_right_and_hard_left_saturate_the_pan() {
        let model = SpatialModel::DEFAULT;
        let right = evaluate([10.0, 0.0, 0.0], &listener(), model).unwrap();
        assert!((right.pan - 1.0).abs() < 1e-6, "pan={}", right.pan);
        let left = evaluate([-10.0, 0.0, 0.0], &listener(), model).unwrap();
        assert!((left.pan + 1.0).abs() < 1e-6, "pan={}", left.pan);
    }

    #[test]
    fn inside_min_distance_is_not_attenuated() {
        let model = SpatialModel {
            min_distance: 5.0,
            max_distance: 30.0,
        };
        let params = evaluate([0.0, 0.0, -4.9], &listener(), model).unwrap();
        assert_eq!(params.gain, 1.0);
    }

    #[test]
    fn beyond_max_distance_is_silent_but_still_panned() {
        let model = SpatialModel::DEFAULT;
        let params = evaluate([100.0, 0.0, 0.0], &listener(), model).unwrap();
        assert_eq!(params.gain, 0.0);
        assert!((params.pan - 1.0).abs() < 1e-6);
    }

    #[test]
    fn illegal_inputs_are_rejected_not_clamped() {
        let model = SpatialModel::DEFAULT;
        // 非有限值
        assert!(matches!(
            evaluate([f32::NAN, 0.0, 0.0], &listener(), model),
            Err(SpatialError::NonFinite("emitter.x"))
        ));
        // 模型反了 / 负数下限
        assert!(matches!(
            evaluate(
                [0.0, 0.0, 0.0],
                &listener(),
                SpatialModel {
                    min_distance: 30.0,
                    max_distance: 10.0
                }
            ),
            Err(SpatialError::InvalidModel(_))
        ));
        assert!(matches!(
            evaluate(
                [0.0, 0.0, 0.0],
                &listener(),
                SpatialModel {
                    min_distance: -1.0,
                    max_distance: 10.0
                }
            ),
            Err(SpatialError::InvalidModel(_))
        ));
        // 退化朝向基：零向量 / 与 up 平行
        assert!(matches!(
            evaluate(
                [0.0, 0.0, 0.0],
                &Listener::new([0.0; 3], [0.0; 3], [0.0, 1.0, 0.0]),
                model
            ),
            Err(SpatialError::DegenerateBasis("forward"))
        ));
        assert!(matches!(
            evaluate(
                [0.0, 0.0, 0.0],
                &Listener::new([0.0; 3], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
                model
            ),
            Err(SpatialError::DegenerateBasis(_))
        ));
    }

    #[test]
    fn folded_positions_stay_inside_rodios_no_attenuation_range() {
        // 折回位置到两耳的距离必须 < 1 m：rodio 的 `Spatial` 用 1/dist² 且 min(1.0)，
        // 超过 1 m 就会二次衰减（这正是"模型权威"要避免的）。
        for pan in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            let (left, right, emitter) = fold_to_ears(&listener(), pan);
            let centre = listener().position;
            let distance = length(sub(emitter, centre));
            assert!(distance < 1.0, "pan={pan} distance={distance}");
            // 声像方向：pan>0 时声源离右耳更近。
            let to_left = length(sub(emitter, left));
            let to_right = length(sub(emitter, right));
            if pan > 0.0 {
                assert!(to_right < to_left, "pan={pan}");
            } else if pan < 0.0 {
                assert!(to_left < to_right, "pan={pan}");
            }
            assert_ne!(left, right, "两耳不能重合（rodio 断言 left != right）");
        }
    }

    #[test]
    fn facing_builds_forward_from_target() {
        let l = Listener::facing([0.0, 0.0, 0.0], [0.0, 0.0, -5.0]);
        assert_eq!(l.forward, [0.0, 0.0, -5.0]);
        // 朝 -Z 与默认一致。
        let params = evaluate([5.0, 0.0, 0.0], &l, SpatialModel::DEFAULT).unwrap();
        assert!((params.pan - 1.0).abs() < 1e-6);
    }
}
