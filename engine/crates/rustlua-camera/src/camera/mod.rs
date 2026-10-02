//! 统一相机：2D 与 3D 的投影、视口、拾取与可见性。
//!
//! ## 一个组件，两个 payload，按职责分子模块
//!
//! "统一相机"统一的是**创作层**，不是表示层：
//!
//! | 层 | 说法 |
//! |---|---|
//! | 场景 / Prefab / 编辑器 / Lua 的**组件** | 只有一个名字 [`crate::camera::CAMERA_COMPONENT`]（`Camera`），2D/3D 由 [`crate::camera::PROJECTION_FIELD`]（`projection=orthographic\|perspective`）判别；旧组件名 `Camera2D`/`Camera3D` **已删除**，仓库级数据门 `shipped_data_uses_only_unified_components` 会拒绝任何写回旧名的场景 |
//! | 引擎**内部表示** | 仍是两个类型：[`crate::camera::Camera2D`]（正交：center/rotation/zoom/bounds/enabled）与 [`crate::camera::Camera3D`]（透视：position/target/fov/near/far），由 [`crate::camera::Camera`] 这个枚举裹在一起携带 |
//!
//! 保留两个内部类型是**刻意的**（docs/54 §1.2「统一抽象，不强行统一所有算法」、§4.2）：
//! `Camera2dController` 的 zoom/bounds/follow/死区/抖动属于正交相机，透视的射线/视锥属于
//! 透视相机，两者共享的字段几乎只有 `enabled`——强行并成一个结构只会让一半字段在另一半
//! 配置下恒为噪声。所以 [`crate::camera::Camera2D`] 是**表示层**用法（ECS 里存的就是它，`script.rs` 把
//! `Camera` 载荷解析成 `Camera` 后按分支插入）；**它不是**、也不能写成场景里的组件名。
//!
//! 各子模块的职责与"不认识什么"：
//!
//! | 子模块 | 职责 | 不认识什么 |
//! |---|---|---|
//! | `component` | 相机类型与数学（逐点换算、矩阵、射线、视锥入口） | 字符串格式、ECS、宿主 |
//! | `codec` | 组件串编解码（场景格式的唯一入口） | 帧循环、宿主 |
//! | `controller` | 2D 跟随 / 偏移 / 死区 / 平滑 / 抖动 + 请求-回填交接 | ECS、渲染 |
//! | `selection` | 哪台相机生效（显式选中 → 排序序第一个可用） | 具体相机类型 |
//! | `view` | 每帧渲染契约（[`crate::camera::Projection`] / [`crate::camera::RenderView`] / [`crate::camera::FrameViews`] / [`crate::camera::ClearPolicy`]） | ECS、宿主 |
//! | `error` | 坐标与相机换算错误（[`crate::camera::CoordinateError`]，`core::math2d` 也在用） | 相机类型之外的东西 |
//!
//! 拆分前这些内容分散在 `math2d::Camera2D` 与 `camera3d::Camera3D` 两处，各自带一套视口/投影/
//! 误差类型，改一处投影要同时想两遍"另一边是不是也这样"。收敛进 `core::camera` 之后再按上述
//! 职责切成子模块：**公开路径一字不变**（全部由本文件 `pub use`），因此这是一次纯搬家。
//!
//! 3D 几何基元（[`rustlua_math::geom3d::Ray3`] / [`rustlua_math::geom3d::Frustum`] / [`rustlua_math::geom3d::Plane`] / [`rustlua_math::geom3d::FrustumClassification`]）已迁到
//! [`rustlua_math::geom3d`]——它们是纯几何，与相机无关（拾取、gizmo、渲染剔除都在用）。
//! 这里保留**兼容 re-export**，既有调用点不受影响；新代码请直接 `use rustlua_math::geom3d::…`。
//!
//! 数值约定（NDC 方向、深度范围 0..1、右手系 -Z 前向、世界 y 向上）由 `tests` 子模块逐条钉住。

mod bake;
mod codec;
mod component;
mod controller;
mod error;
mod selection;
mod view;

/// 烘焙**计数账本**（拆分批 8）：只存在于测试构建。它把"宿主每帧烘焙一次"这句口径变成可判的
/// 数字——现实是"每个消费者每帧各一次"（详见模块文档）。
pub mod ledger;

pub use bake::{CameraView, OrthoTerms};
pub use codec::{Camera2dComponentError, Camera3dComponentError};
pub use component::{
    CAMERA_COMPONENT, Camera, Camera2D, Camera3D, Camera3dError, CameraBounds, CameraProjection,
    DEFAULT_2D_CAMERA_CENTER, PROJECTION_FIELD, Viewport,
};
pub use controller::Camera2dController;
pub use error::CoordinateError;
pub use selection::CameraSelection;
pub use view::{
    ClearPolicy, DEFAULT_3D_CLEAR, FrameViews, ORTHO_FAR, ORTHO_NEAR, Projection, RenderView,
};

// 几何基元的兼容 re-export（`core::camera::Ray3` 等旧路径仍然有效）。
pub use rustlua_math::geom3d::{Frustum, FrustumClassification, Plane, Ray3};

#[cfg(test)]
mod tests;
