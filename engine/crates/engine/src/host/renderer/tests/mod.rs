//! `Renderer` 的测试集合：WP12 阶段 4a 从 `renderer/mod.rs` 末尾的 `#[cfg(test)] mod tests`
//! 原样搬来，按域分成 7 个文件（源文件 6,089 行里这一段占 1,847 行，超过计划 §2 的 1,500 行
//! 上限）。断言、造数据与离屏渲染路径一字未改。
//!
//! 可见性依据（计划 §3.1.5）：本目录里的每个模块都是 `renderer` 的后代，`use super::*;` 的
//! glob 会把父模块的**私有项**连同本模块的 `use` 一并带下来，因此拆分没有放宽任何可见性；
//! 原先声明在本模块顶部的 helper 也仍在父作用域可见（本目录内没有测试之外的 helper）。
//! 这行措辞是刻意的：写成属性字面量会让 `grep -c '#\[test\]'` 的计数虚高 1。

use super::*;
use crate::core::camera::{Camera2D, ClearPolicy, DEFAULT_3D_CLEAR, Projection, RenderView};
use crate::core::math3d::Quat;
use crate::core::transform::Vec2;

use crate::host::frame::OverlayVertex;
// WP12 阶段 4d：这两个名字原先由 `renderer/mod.rs` 顶部的 use 块带进来（测试靠 glob 取用），
// 构造代码搬走后 `mod.rs` 不再需要它们，就在这里显式声明。
use crate::host::frame::WorldSpriteVertex;
use wgpu::DeviceDescriptor;

mod capacity_and_caches;
mod depth_and_frame_views;
mod offscreen_and_tetris;
mod passes;
mod readback;
mod render_metrics;
mod world_sprite;
