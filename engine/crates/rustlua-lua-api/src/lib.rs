//! Lua 绑定的**契约层**：能力句柄、安装器注册表、绑定期错误口径。
//!
//! ## 为什么单独一个 crate
//!
//! `rustlua-scripting` 曾经正向依赖全部 13 个兄弟 crate（`docs/62` 的 P1「依赖倒置」）：
//! 它既是 VM 宿主，又是 270+ 个绑定的家。结果是任何领域 crate 改动都会重编 VM 与全部绑定，
//! 改动半径等于拆分前的整个 core。
//!
//! 倒置的障碍是**类型**：绑定实现要认识 `Rc<RefCell<ScriptState>>` 这个 40 字段中枢，
//! 一旦把绑定搬到 `rustlua-audio`，audio 就得依赖 scripting → 成环。
//!
//! 这一层把"绑定需要的、与具体领域无关的东西"固定下来，变成双方共同依赖的**叶子**：
//!
//! ```text
//!             rustlua-lua-api   （本 crate：无领域依赖）
//!               ↑          ↑
//!   rustlua-audio          rustlua-scripting
//!        ↑                        ↑
//!        └──────── engine ────────┘   （组合根：把句柄交给安装器）
//! ```
//!
//! - [`Handle`]：宿主拥有的能力单元（如 `AudioSystem`）。绑定借它，宿主读写它；
//!   重入借用返回 Lua 错误而不是 panic（与 `rustlua-scripting` 的 `lua_borrow` 同口径）。
//! - [`Registry`]/[`Installer`]：绑定集合的注册表。领域 crate 提供安装器，组合根把它塞进
//!   运行时的 VM 装配流程——`rustlua-scripting` 只认注册表，不认具体 crate。
//!
//! ## 红线
//!
//! 本 crate **不得**依赖任何 `rustlua-<领域>` crate（只允许 mlua）。一旦破例，所有引用它的
//! 领域 crate 都会跟着被拖进编译图，倒置就白做了；`engine` 的架构耦合门会盯住这条。

use mlua::Result as LuaResult;

mod cell;
pub mod values;
mod registry;

pub use cell::Handle;
pub use registry::{Installer, Registry};

/// 绑定期错误：Lua 侧看到的是 `RuntimeError`，与 `rustlua-scripting::api_error` 同一种形状。
///
/// 领域 crate 的绑定只有这一种错误出口，所以不必依赖 scripting 的错误模块。
pub fn api_error(message: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(message.into())
}

/// 重入借用的统一报错（`Handle` 内部用；与 scripting 的 `reentrant_borrow` 同一句话）。
///
/// 为什么要这个形状：同一个能力在两个 Lua 调用之间被借两次，一定是绑定里又回调了 Lua。
/// 沉默地 panic 会让真机只看到 `unreachable`（release 下 strip 了符号），所以必须变成
/// 带上下文的 Lua 错误。
pub fn reentrant_borrow(what: &str, error: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::external(format!(
        "{what} is already borrowed (reentrant Lua call?): {error}"
    ))
}

/// `LuaResult` 的再导出（领域 crate 不必各自 `use mlua::Result`）。
pub type Result<T> = LuaResult<T>;
