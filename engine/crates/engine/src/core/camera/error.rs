//! 坐标与相机换算的共用错误。
//!
//! [`CoordinateError`] 是相机换算与 2D 坐标工具（`core::math2d`）共用的错误类型，因此它既
//! 由本模块定义，也被 `core::math2d` 重新导出（`pub use crate::core::camera::CoordinateError`）。
//! 搬迁/重命名这个类型会改动 `math2d` 的公开 API，别顺手改。

use std::{error::Error, fmt};

use crate::core::transform::Vec2;

/// Errors produced by coordinate and camera conversions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CoordinateError {
    InvalidViewport {
        width: f32,
        height: f32,
    },
    InvalidZoom(f32),
    NonFiniteVector {
        name: &'static str,
        value: Vec2,
    },
    NonFiniteColor {
        value: [f32; 4],
    },
    InvalidRotation(f32),
    InvalidBounds {
        min: Vec2,
        max: Vec2,
    },
    /// 深度范围非法（`far <= near` 或含非有限值）。
    InvalidDepthRange {
        near: f32,
        far: f32,
    },
}

impl fmt::Display for CoordinateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidViewport { width, height } => {
                write!(formatter, "invalid viewport: {width}x{height}")
            }
            Self::InvalidZoom(zoom) => write!(formatter, "invalid camera zoom: {zoom}"),
            Self::NonFiniteVector { name, value } => write!(
                formatter,
                "{name} vector is not finite: ({}, {})",
                value.x, value.y
            ),
            Self::NonFiniteColor { value } => write!(formatter, "color is not finite: {value:?}"),
            Self::InvalidRotation(rotation) => {
                write!(formatter, "invalid camera rotation: {rotation}")
            }
            Self::InvalidBounds { min, max } => {
                write!(formatter, "invalid camera bounds: {min:?}..{max:?}")
            }
            Self::InvalidDepthRange { near, far } => {
                write!(formatter, "invalid depth range: near={near} far={far}")
            }
        }
    }
}
impl Error for CoordinateError {}
