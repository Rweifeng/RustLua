//! 坐标与相机换算的共用错误。
//!
//! `CoordinateError` 定义在 [`rustlua_math::math2d`]（它是 math2d 颜色校验与相机换算的共用
//! 错误），本模块保留 re-export，既有 `camera::CoordinateError` 路径不受影响。

pub use rustlua_math::math2d::CoordinateError;
