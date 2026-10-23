//! 结构化图错误：解析、校验、生成共用一套，编辑器错误列表可直接定位到节点/端口。

use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq)]
pub enum GraphError {
    #[error("vgraph 解析失败（line {line}, column {column}）：{message}")]
    Parse {
        line: usize,
        column: usize,
        message: String,
    },
    #[error("不支持的 .vgraph 版本 {found}（只接受 {expected}）")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("图的 name 为空")]
    EmptyGraphName,
    #[error("图里没有任何事件节点（on_start/on_update/on_interact/on_collision 至少一个）")]
    MissingEvent,
    #[error("节点 id `{0}` 非法：只允许 1–64 个 [A-Za-z0-9_-] 字符")]
    InvalidNodeId(String),
    #[error("重复节点 id `{0}`")]
    DuplicateNodeId(String),
    #[error("节点 id `{first}` 与 `{second}` 归一化后都变成 `{name}`，生成代码会撞名")]
    NodeIdCollision {
        first: String,
        second: String,
        name: String,
    },
    #[error("节点 `{node}` 的类型 `{kind}` 未注册")]
    UnknownNodeType { node: String, kind: String },
    #[error("事件入口重复：节点 `{node}` 与 `{first}` 都是 `{kind}`")]
    DuplicateEvent {
        node: String,
        first: String,
        kind: String,
    },
    #[error("节点 `{node}` 的坐标必须是有限数")]
    InvalidPosition { node: String },
    #[error("节点 `{node}` 缺少必填参数 `{param}`")]
    MissingParam { node: String, param: String },
    #[error("节点 `{node}` 有未知参数 `{param}`")]
    UnknownParam { node: String, param: String },
    #[error("节点 `{node}` 的参数 `{param}` 无效：{message}")]
    InvalidParam {
        node: String,
        param: String,
        message: String,
    },
    #[error("边悬空：`{from_node}.{from_port}` → `{to_node}.{to_port}`：{message}")]
    DanglingEdge {
        from_node: Box<str>,
        from_port: Box<str>,
        to_node: Box<str>,
        to_port: Box<str>,
        message: Box<str>,
    },
    #[error("端口不存在：节点 `{node}` 没有{side}端口 `{port}`")]
    UnknownPort {
        node: String,
        port: String,
        side: &'static str,
    },
    #[error("重复边：`{from_node}.{from_port}` → `{to_node}.{to_port}`")]
    DuplicateEdge {
        from_node: String,
        from_port: String,
        to_node: String,
        to_port: String,
    },
    #[error("数据输入端口 `{node}.{port}` 有多条入边（数据只能接一条；流程输入允许汇合）")]
    DuplicateInputEdge { node: String, port: String },
    #[error("流程输出 `{node}.{port}` 扇出到多个节点（{targets:?}）；用 sequence 显式排序")]
    FlowFanOut {
        node: String,
        port: String,
        targets: Vec<String>,
    },
    #[error(
        "类型不匹配：`{from_node}.{from_port}` 是 {from_type}，`{to_node}.{to_port}` 需要 {to_type}"
    )]
    TypeMismatch {
        from_node: Box<str>,
        from_port: Box<str>,
        from_type: &'static str,
        to_node: Box<str>,
        to_port: Box<str>,
        to_type: &'static str,
    },
    #[error("节点 `{node}` 的必填输入 `{port}` 未连接")]
    MissingInput { node: String, port: String },
    #[error("数据环：{}（数据节点不能互相依赖）", nodes.join(" → "))]
    DataCycle { nodes: Vec<String> },
    #[error("流程环没有 gate/sequence 守卫：{}", nodes.join(" → "))]
    FlowCycle { nodes: Vec<String> },
    #[error("变量名 `{name}` 重复")]
    DuplicateVariable { name: String },
    #[error("变量名 `{name}` 非法：只允许 [A-Za-z_][A-Za-z0-9_]*，且不能是 Lua 关键字")]
    InvalidVariableName { name: String },
    #[error("变量 `{name}` 的默认值与类型 {value_type} 不匹配")]
    InvalidVariableDefault {
        name: String,
        value_type: &'static str,
    },
    #[error("图过大：{nodes} 个节点（上限 {limit}）")]
    GraphTooLarge { nodes: usize, limit: usize },
}
