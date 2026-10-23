//! P3-E03-1：可视化脚本的**格式 + 类型检查 + 确定性 Lua 代码生成**（无编辑器 UI）。
//!
//! ## 资产
//!
//! `.vgraph` 是 JSON 文本：`version`、`name`、`variables`、`nodes`、`edges`。节点 id 稳定，
//! 生成 Lua 的局部变量名由 id 派生；位置与参数原样往返，编辑器（E03-2）只改这份文件。
//!
//! ## 校验口径（全部一次性收齐）
//!
//! - 重复 id、id 字符集、以及 `a-b` / `a_b` 归一化后撞名；
//! - 悬空边（起终点不存在、端口不存在）、重复边、输入多条入边；
//! - 端口类型不匹配（Flow 与数据互不相通；`Any` 兼容任意类型）；
//! - 必填输入未连接（含动作的 `in`）；
//! - 数据环拒绝；流程环必须经过 `gate`/`sequence` 才放行；
//! - 每种事件（start/update/interact/collision）最多一个入口，且至少一个事件。
//!
//! ## 生成契约
//!
//! - 每个事件生成 `M.on_start(owner)` / `M.on_update(owner, dt)` /
//!   `M.on_interact(owner, target)` / `M.on_collision(owner, other)`；
//! - 文件头固定 `-- generated from <graph>; do not edit`，图是唯一源；
//! - 动作只调用现有 Lua API（`transform3d.set`、`animation3d.set`、`audio.play`、
//!   `event.emit`、`ui.text`、`save.commit`），不引入第二套 VM；
//! - 生成物不写盘：E03-3 接到 `scripts/generated/<name>.lua` 并走现有加载/热重载。
//!
//! 节点集是 v1 的 18 个内置节点；`spawn_prefab` / `get_component` 因当前 Lua API 面
//! 没有稳定函数而暂缓。E03-4 起插件节点与内置节点共用 [`registry::node_spec`] 查找表：
//! [`registry::register_plugin_node`] 在注册期校验撞名/分类/模板占位符，生成器按模板展开
//! 输入表达式与参数（样例见 `tests/plugin_nodes.rs`）。

mod codegen;
mod error;
mod graph;
mod registry;
mod validate;

pub use codegen::{compile, compile_traced};
pub use error::GraphError;
pub use graph::{
    Edge, Graph, Node, Params, PortRef, VGRAPH_VERSION, ValueType, Variable, is_valid_node_id,
    lua_var_name,
};
pub use registry::{
    Category, EventKind, NodeSpec, ParamSpec, PluginNode, PortSpec, PortType, all_nodes, node_spec,
    plugin_node, register_plugin_node,
};
pub use validate::{MAX_NODES, validate};
