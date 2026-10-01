//! 坐标与相机换算的共用错误。
//!
//! [`crate::core::camera::CoordinateError`] 是相机换算与 2D 坐标工具（`core::math2d`）共用的错误类型，因此它既
//! 由本模块定义，也被 `core::math2d` 重新导出（`pub use crate::core::camera::CoordinateError`）。
//! 搬迁/重命名这个类型会改动 `math2d` 的公开 API，别顺手改。

use crate::core::transform::Vec2;

/// Errors produced by coordinate and camera conversions.
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum CoordinateError {
    #[error("invalid viewport: {width}x{height}")]
    InvalidViewport { width: f32, height: f32 },
    #[error("invalid camera zoom: {0}")]
    InvalidZoom(f32),
    #[error("{name} vector is not finite: ({}, {})", .value.x, .value.y)]
    NonFiniteVector { name: &'static str, value: Vec2 },
    #[error("color is not finite: {value:?}")]
    NonFiniteColor { value: [f32; 4] },
    #[error("invalid camera rotation: {0}")]
    InvalidRotation(f32),
    #[error("invalid camera bounds: {min:?}..{max:?}")]
    InvalidBounds { min: Vec2, max: Vec2 },
    /// 深度范围非法（`far <= near` 或含非有限值）。
    #[error("invalid depth range: near={near} far={far}")]
    InvalidDepthRange { near: f32, far: f32 },
}
