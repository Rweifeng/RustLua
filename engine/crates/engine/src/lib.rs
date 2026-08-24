//! Rust + Lua 游戏引擎。
//!
//! - [`core`]：纯逻辑模块（ECS、场景、Lua、渲染数据、资源、物理等），
//!   无桌面窗口/GPU 依赖，可被任意宿主复用。
//! - [`host`]：GPU 渲染与运行时场景（wgpu/winit），消费 `core` 产生的帧数据。

// 维护注意：`core` 模块名会遮蔽标准库 `core` crate，本 crate 内请使用 `std::`/完整路径，
// 不要写裸的 `core::...`。
pub mod core;
pub mod host;

/// 便捷再导出：`engine::version()` 与 `engine::PROJECT_WORK_DIR`。
pub use core::{PROJECT_WORK_DIR, version};
