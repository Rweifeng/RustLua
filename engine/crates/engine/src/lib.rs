//! Rust + Lua 游戏引擎。
//!
//! - [`host`]：GPU 渲染与运行时场景（wgpu/winit），消费引擎子 crate 产生的帧数据。
//! - [`lua_api`]：Lua 侧脚本契约的**文档页**（按模块分节，内容自动生成、说明人工补）。

pub mod host;
pub mod lua_api;

/// 便捷再导出：`engine::version()` 与 `engine::PROJECT_WORK_DIR`。
pub use rustlua_infra::{PROJECT_WORK_DIR, version};
