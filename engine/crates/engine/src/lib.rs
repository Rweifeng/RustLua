//! Rust + Lua 游戏引擎。
//!
//! - [`core`]：纯逻辑模块（ECS、场景、Lua、渲染数据、资源、物理等），
//!   无桌面窗口/GPU 依赖，可被任意宿主复用。
//! - [`host`]：GPU 渲染与运行时场景（wgpu/winit），消费 `core` 产生的帧数据。
//! - [`lua_api`]：Lua 侧脚本契约的**文档页**（按模块分节，内容自动生成、说明人工补）。

// 维护注意：`core` 模块名会遮蔽标准库 `core` crate，本 crate 内请使用 `std::`/完整路径，
// 不要写裸的 `core::...`。
pub mod core;
pub mod host;
pub mod lua_api;

/// 便捷再导出：`engine::version()` 与 `engine::PROJECT_WORK_DIR`。
pub use core::{PROJECT_WORK_DIR, version};
