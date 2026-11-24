//! 第三方 runtime 隔离层。只有这里允许出现 `spine2d` 的类型；
//! 上层（player / render / 组件 / Lua）一律只认 `crate::model` 的中性类型。
//! 升级 Spine 4.4/5.x 或换运行时时只替换本模块。

mod spine2d_backend;

pub use spine2d_backend::{BackendData, BackendInstance};
