//! AI tool 面：**唯一**的工具契约来源。
//!
//! `ai_tool_surface.json` 由 [`surface_json`] 生成；测试把它与仓库基线逐字节比对。
//! 新增/删除/改签名必须先让快照门变红，再显式刷新基线（等价于一次 API review）。

use crate::ui_tools::names as ui;
use serde_json::{Value, json};
use std::sync::OnceLock;

/// 工具风险层（docs/71 §7.1）。
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub enum ToolTier {
    /// 只读查询，免确认。
    #[serde(rename = "T0")]
    Read,
    /// 场景内写，一次提交 = 一条 Undo。
    #[serde(rename = "T1")]
    T1,
    /// 破坏性/跨文件写，每次显式确认。
    #[serde(rename = "T2")]
    T2,
    /// 工作区/会话级，超出 Undo 栈，每次显式确认 + 无头拒绝。
    #[serde(rename = "T3")]
    T3,
}

impl ToolTier {
    pub const fn is_write(self) -> bool {
        !matches!(self, Self::Read)
    }
}

#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub tier: ToolTier,
    pub input_schema: Value,
}

#[derive(Clone, Debug)]
pub struct McpResource {
    pub uri: &'static str,
    pub name: &'static str,
    pub mime_type: &'static str,
    pub description: &'static str,
}

/// 创建 UI 预设的机器可读契约：节点类型、锚点、样式属性、完整示例与工具调用顺序。
/// 由 MCP resource `rustlua://ui/guide` 提供，客户端不需要把这份契约硬编码进提示词。
pub const UI_GUIDE: &str = include_str!("ui_guide.md");

const RESOURCES: &[McpResource] = &[
    McpResource {
        uri: "rustlua://lua-api/surface",
        name: "Lua API surface",
        mime_type: "application/json",
        description: "Lua API 行情的机器可读基线（模块 → 函数名），供生成 Lua 时对齐真签名。",
    },
    McpResource {
        uri: "rustlua://lua-api/docs",
        name: "Lua API docs",
        mime_type: "text/markdown",
        description: "由 API 面生成的文档桩（模块/函数清单）。",
    },
    McpResource {
        uri: "rustlua://components",
        name: "Component registry",
        mime_type: "application/json",
        description: "可 add_component 的组件名与类别。",
    },
    McpResource {
        uri: "rustlua://scene/schema",
        name: "Scene component schema",
        mime_type: "application/json",
        description: "组件字段 schema（类型、必填、候选值、领域规则）。",
    },
    McpResource {
        uri: "rustlua://project/config",
        name: "Project config",
        mime_type: "application/toml",
        description: "当前工程的 project.toml 原始文本。",
    },
    McpResource {
        uri: "rustlua://ui/guide",
        name: "UI preset guide",
        mime_type: "text/markdown",
        description: "UiSpec 契约、能力矩阵、完整示例与「校验→预览→创建」调用顺序，供一句话创建 UI 预设。",
    },
];

fn schema_object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn entity_id(description: &str) -> Value {
    json!({"type": "integer", "minimum": 0, "description": description})
}

fn build_tools() -> Vec<ToolSpec> {
    vec![
        // ── 读 tool（T0）────────────────────────────────────────────────
        ToolSpec {
            name: "get_scene_tree",
            description: "返回当前场景的实体树（id、名称、父节点、组件名、子节点）。",
            tier: ToolTier::Read,
            input_schema: schema_object(json!({}), &[]),
        },
        ToolSpec {
            name: "get_selection",
            description: "返回当前选区（主选中 + 全部选中实体）。",
            tier: ToolTier::Read,
            input_schema: schema_object(json!({}), &[]),
        },
        ToolSpec {
            name: "list_components",
            description: "列出可添加的组件名与类别（add_component 的取值域）。",
            tier: ToolTier::Read,
            input_schema: schema_object(json!({}), &[]),
        },
        ToolSpec {
            name: "get_inspector_schema",
            description: "返回组件字段 schema。给 entity 时附带该实体各字段当前取值。",
            tier: ToolTier::Read,
            input_schema: schema_object(
                json!({
                    "entity": entity_id("可选：读取当前取值的实体 id"),
                    "component": {"type": "string", "description": "可选：只返回该组件"}
                }),
                &[],
            ),
        },
        ToolSpec {
            name: "read_console",
            description: "读取编辑器控制台最近的日志行（按时间正序）。",
            tier: ToolTier::Read,
            input_schema: schema_object(
                json!({
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 100},
                    "contains": {"type": "string", "description": "可选：只保留包含该子串的行"}
                }),
                &[],
            ),
        },
        ToolSpec {
            name: "list_lua_api",
            description: "列出 Lua API 表面（模块 → 函数名）。",
            tier: ToolTier::Read,
            input_schema: schema_object(json!({}), &[]),
        },
        ToolSpec {
            name: "get_lua_api",
            description: "查询 Lua API 签名/文档桩；module 与 function 均可选。",
            tier: ToolTier::Read,
            input_schema: schema_object(
                json!({
                    "module": {"type": "string"},
                    "function": {"type": "string"}
                }),
                &[],
            ),
        },
        ToolSpec {
            name: "eval_lua_readonly",
            description: "在隔离的只读 Lua VM 中求值表达式并按 JSON 返回结果；不接触游戏运行态。",
            tier: ToolTier::Read,
            input_schema: schema_object(
                json!({"expression": {"type": "string", "minLength": 1}}),
                &["expression"],
            ),
        },
        ToolSpec {
            name: "capture_screenshot",
            description: "截取编辑器窗口（仅 GUI 运行时可用的 PNG 路径）。",
            tier: ToolTier::Read,
            input_schema: schema_object(json!({}), &[]),
        },
        ToolSpec {
            name: "get_project_config",
            description: "返回当前工程根路径与 project.toml 文本。",
            tier: ToolTier::Read,
            input_schema: schema_object(json!({}), &[]),
        },
        // ── 写 tool（T1–T3）──────────────────────────────────────────────
        ToolSpec {
            name: "new_scene",
            description: "新建空场景。T3：会清空 Undo 栈，必须显式确认。",
            tier: ToolTier::T3,
            input_schema: schema_object(
                json!({"name": {"type": "string", "minLength": 1}}),
                &["name"],
            ),
        },
        ToolSpec {
            name: "open_scene",
            description: "打开场景/Prefab/Widget 文件。T3：脏文档会挂起等待确认，无头拒绝。",
            tier: ToolTier::T3,
            input_schema: schema_object(
                json!({"path": {"type": "string", "minLength": 1, "description": "工程内相对路径"}}),
                &["path"],
            ),
        },
        ToolSpec {
            name: "save_scene",
            description: "保存当前场景到既有路径。T3：落盘不可撤销。",
            tier: ToolTier::T3,
            input_schema: schema_object(json!({}), &[]),
        },
        ToolSpec {
            name: "save_scene_as",
            description: "另存当前场景到新路径。T3：落盘不可撤销。",
            tier: ToolTier::T3,
            input_schema: schema_object(
                json!({"path": {"type": "string", "minLength": 1}}),
                &["path"],
            ),
        },
        ToolSpec {
            name: "undo",
            description: "撤销上一条编辑（T1，可重放）。",
            tier: ToolTier::T1,
            input_schema: schema_object(json!({}), &[]),
        },
        ToolSpec {
            name: "redo",
            description: "重做上一条撤销（T1）。",
            tier: ToolTier::T1,
            input_schema: schema_object(json!({}), &[]),
        },
        ToolSpec {
            name: "set_play_state",
            description: "控制 Play 会话：start/stop/pause/resume/restart（T3）。",
            tier: ToolTier::T3,
            input_schema: schema_object(
                json!({"action": {"type": "string", "enum": ["start", "stop", "pause", "resume", "restart"]}}),
                &["action"],
            ),
        },
        ToolSpec {
            name: "step_play",
            description: "暂停态下单帧步进（T3）。",
            tier: ToolTier::T3,
            input_schema: schema_object(
                json!({"frames": {"type": "integer", "minimum": 1, "maximum": 600, "default": 1}}),
                &[],
            ),
        },
        ToolSpec {
            name: "create_entity",
            description: "创建实体（可选父节点与 3D 变换），返回新实体 id。T1。",
            tier: ToolTier::T1,
            input_schema: schema_object(
                json!({
                    "name": {"type": "string"},
                    "parent": entity_id("可选父实体 id"),
                    "transform_3d": {"type": "boolean", "default": false}
                }),
                &[],
            ),
        },
        ToolSpec {
            name: "select_entity",
            description: "把选区设为单个实体。T1。",
            tier: ToolTier::T1,
            input_schema: schema_object(json!({"id": entity_id("目标实体 id")}), &["id"]),
        },
        ToolSpec {
            name: "rename_entity",
            description: "重命名实体。T1。",
            tier: ToolTier::T1,
            input_schema: schema_object(
                json!({
                    "entity": entity_id("目标实体 id"),
                    "name": {"type": "string", "minLength": 1}
                }),
                &["entity", "name"],
            ),
        },
        ToolSpec {
            name: "duplicate_entity",
            description: "复制实体（含子树）。显式实体优先，缺省用当前选区。T1。",
            tier: ToolTier::T1,
            input_schema: schema_object(json!({"entity": entity_id("可选：要复制的实体 id")}), &[]),
        },
        ToolSpec {
            name: "delete_entity",
            description: "删除实体（含子树），一次提交一条 Undo。T2。",
            tier: ToolTier::T2,
            input_schema: schema_object(
                json!({"entities": {"type": "array", "items": {"type": "integer", "minimum": 0}, "minItems": 1}}),
                &["entities"],
            ),
        },
        ToolSpec {
            name: "reparent_entity",
            description: "把实体移到新父节点/场景根。T1。",
            tier: ToolTier::T1,
            input_schema: schema_object(
                json!({
                    "entity": entity_id("要移动的实体 id"),
                    "target": {"type": ["integer", "null"], "minimum": 0, "description": "新父节点；null 表示场景根"},
                    "position": {"type": "string", "enum": ["before", "after", "child"], "default": "child"}
                }),
                &["entity"],
            ),
        },
        ToolSpec {
            name: "add_component",
            description: "给实体添加注册表组件（默认值）。T1。",
            tier: ToolTier::T1,
            input_schema: schema_object(
                json!({
                    "entity": entity_id("目标实体 id"),
                    "component": {"type": "string", "minLength": 1}
                }),
                &["entity", "component"],
            ),
        },
        ToolSpec {
            name: "remove_component",
            description: "移除实体上的组件。T1。",
            tier: ToolTier::T1,
            input_schema: schema_object(
                json!({
                    "entity": entity_id("目标实体 id"),
                    "component": {"type": "string", "minLength": 1}
                }),
                &["entity", "component"],
            ),
        },
        ToolSpec {
            name: "set_component_fields",
            description: "原子写入组件字段（一次提交一条 Undo）。entities 可选，缺省为当前选区。T1。",
            tier: ToolTier::T1,
            input_schema: schema_object(
                json!({
                    "entities": {"type": "array", "items": {"type": "integer", "minimum": 0}, "minItems": 1},
                    "component": {"type": "string", "minLength": 1},
                    "fields": {"type": "object", "additionalProperties": {"type": ["string", "number", "integer", "boolean"]}, "minProperties": 1}
                }),
                &["component", "fields"],
            ),
        },
        ToolSpec {
            name: "apply_prefab_override",
            description: "把实例字段写回源 Prefab（T1）。",
            tier: ToolTier::T1,
            input_schema: schema_object(
                json!({
                    "entity": entity_id("Prefab 实例 id"),
                    "field": {"type": "string", "minLength": 1}
                }),
                &["entity", "field"],
            ),
        },
        ToolSpec {
            name: "revert_prefab_override",
            description: "把实例字段还原为源 Prefab 取值（T1）。",
            tier: ToolTier::T1,
            input_schema: schema_object(
                json!({
                    "entity": entity_id("Prefab 实例 id"),
                    "field": {"type": "string", "minLength": 1}
                }),
                &["entity", "field"],
            ),
        },
        ToolSpec {
            name: "revert_prefab_overrides",
            description: "回退实例上的全部覆盖（T2）。",
            tier: ToolTier::T2,
            input_schema: schema_object(
                json!({"entity": entity_id("Prefab 实例 id")}),
                &["entity"],
            ),
        },
        ToolSpec {
            name: "reapply_prefab",
            description: "重新应用源 Prefab（T1，仅根实例）。",
            tier: ToolTier::T1,
            input_schema: schema_object(
                json!({"entity": entity_id("Prefab 根实例 id")}),
                &["entity"],
            ),
        },
        ToolSpec {
            name: "save_prefab_to_source",
            description: "把实例写回源 Prefab 文件并更新全部实例（T2，跨文件）。",
            tier: ToolTier::T2,
            input_schema: schema_object(
                json!({"entity": entity_id("Prefab 根实例 id")}),
                &["entity"],
            ),
        },
        ToolSpec {
            name: "unpack_prefab",
            description: "解包 Prefab 实例，断开与源的关系（T2）。",
            tier: ToolTier::T2,
            input_schema: schema_object(
                json!({"entity": entity_id("Prefab 实例 id")}),
                &["entity"],
            ),
        },
        ToolSpec {
            name: "write_lua_script",
            description: "校验并写入工程内 Lua 脚本。先语法/API 校验，失败返回错误供 AI 修复。T2。",
            tier: ToolTier::T2,
            input_schema: schema_object(
                json!({
                    "path": {"type": "string", "minLength": 1, "description": "工程内相对路径，.lua"},
                    "code": {"type": "string", "minLength": 1}
                }),
                &["path", "code"],
            ),
        },
        ToolSpec {
            name: "open_project",
            description: "把编辑器切到另一个工程目录（一份 stdio 会话服务多个工程，不必每个工程开一条连接器）。path 可为绝对路径，或相对当前工程根的路径（允许 ..）。切换是后台导入：本调用只代表「已排队」，落地后用 get_project_config 的 root 确认。脏文档会挂起等编辑器内确认，无头拒绝。T3。",
            tier: ToolTier::T3,
            input_schema: schema_object(
                json!({"path": {"type": "string", "minLength": 1, "description": "目标工程根目录"}}),
                &["path"],
            ),
        },
        // ── UI 文档 tool（M76 契约；执行体在 M76-03）──────────────────────
        ToolSpec {
            name: ui::EXPORT_SPEC,
            description: "导出 UI 文档的 UiSpec 与 revision。T0。",
            tier: ToolTier::Read,
            input_schema: schema_object(
                json!({"document": {"type": "string", "minLength": 1, "description": "工程内 .ui.json 相对路径"}}),
                &["document"],
            ),
        },
        ToolSpec {
            name: ui::VALIDATE_SPEC,
            description: "按运行时能力矩阵校验 UiSpec，返回诊断（不落盘）。T0。",
            tier: ToolTier::Read,
            input_schema: schema_object(
                json!({"spec": {"type": "object", "description": "UiSpec JSON"}}),
                &["spec"],
            ),
        },
        ToolSpec {
            name: ui::PREVIEW_SPEC,
            description: "离屏渲染 UiSpec 预览图（PNG 字节）。T0。",
            tier: ToolTier::Read,
            input_schema: schema_object(
                json!({
                    "spec": {"type": "object", "description": "UiSpec JSON"},
                    "width": {"type": "integer", "minimum": 1, "maximum": 8192, "default": 1280},
                    "height": {"type": "integer", "minimum": 1, "maximum": 8192, "default": 720},
                    "scale": {"type": "number", "minimum": 0.1, "maximum": 8.0, "default": 1.0}
                }),
                &["spec"],
            ),
        },
        ToolSpec {
            name: ui::LIST_ASSETS,
            description: "列出 UI 资产及其 provenance（未审阅/生成/人工）。T0。",
            tier: ToolTier::Read,
            input_schema: schema_object(
                json!({"document": {"type": "string", "description": "可选：只列出该文档引用的资产"}}),
                &[],
            ),
        },
        ToolSpec {
            name: ui::APPLY_SPEC,
            description: "把 UiSpec 编译为提案（diff+预览+成本）；接受后一次事务、一条 Undo。T1。",
            tier: ToolTier::T1,
            input_schema: schema_object(
                json!({
                    "document": {"type": "string", "minLength": 1, "description": "工程内 .ui.json 相对路径"},
                    "base_revision": {"type": "integer", "minimum": 0},
                    "spec": {"type": "object", "description": "UiSpec JSON"}
                }),
                &["document", "base_revision", "spec"],
            ),
        },
        ToolSpec {
            name: ui::APPLY_PATCH,
            description: "把 UiPatch 应用到指定 revision 并生成提案；base_revision 过期直接拒绝。T1。",
            tier: ToolTier::T1,
            input_schema: schema_object(
                json!({
                    "document": {"type": "string", "minLength": 1, "description": "工程内 .ui.json 相对路径"},
                    "patch": {"type": "object", "description": "UiPatch JSON（含 base_revision 与 ops）"}
                }),
                &["document", "patch"],
            ),
        },
        ToolSpec {
            name: ui::CREATE_PRESET,
            description: "创建**新**的 .ui.json UI 预设（只新建；已存在会拒绝）。用户要求「做一个商店/背包/对话框界面」时：先读 resource rustlua://ui/guide 构造 UiSpec，用 ui_validate_spec 修到 0 error，再调用本工具；落盘后返回 revision 与预览 PNG（MCP image 内容块）。T1。",
            tier: ToolTier::T1,
            input_schema: schema_object(
                json!({
                    "document": {"type": "string", "minLength": 1, "description": "工程内 .ui.json 相对路径，例如 ui/tavern.ui.json"},
                    "spec": {"type": "object", "description": "UiSpec JSON（见 rustlua://ui/guide）"},
                    "preview": {"type": "boolean", "default": true, "description": "是否返回预览 PNG"},
                    "width": {"type": "integer", "minimum": 1, "maximum": 8192, "default": 640},
                    "height": {"type": "integer", "minimum": 1, "maximum": 8192, "default": 360},
                    "scale": {"type": "number", "minimum": 0.1, "maximum": 8.0, "default": 1.0}
                }),
                &["document", "spec"],
            ),
        },
        ToolSpec {
            name: ui::PROMOTE_ASSET,
            description: "把审阅通过的资产生成登记提升进工程并写入 provenance。T2。",
            tier: ToolTier::T2,
            input_schema: schema_object(
                json!({
                    "asset_id": {"type": "string", "minLength": 1},
                    "reviewed": {"type": "boolean", "description": "人工审阅结果；false 不得提升"},
                    "license": {"type": "string", "description": "许可标识，发布清单必填"},
                    "used_by": {"type": "array", "items": {"type": "string"}}
                }),
                &["asset_id", "reviewed"],
            ),
        },
        ToolSpec {
            name: ui::GENERATE_ASSETS,
            description: "按 AssetPlan 生成候选图；逐项预算与确认，超限拒绝。T2。",
            tier: ToolTier::T2,
            input_schema: schema_object(
                json!({
                    "plan": {"type": "array", "items": {"type": "object"}, "minItems": 1},
                    "max_images": {"type": "integer", "minimum": 1, "maximum": 64}
                }),
                &["plan"],
            ),
        },
        ToolSpec {
            name: ui::SLICE_ASSET,
            description: "按手工矩形从源 PNG 裁片并写 slices.json（写工程文件）。T2。",
            tier: ToolTier::T2,
            input_schema: schema_object(
                json!({
                    "source": {"type": "string", "minLength": 1, "description": "工程内源 PNG 相对路径"},
                    "output_dir": {"type": "string", "minLength": 1, "description": "裁片与 slices.json 的输出目录"},
                    "regions": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string", "minLength": 1},
                                "rect": {
                                    "type": "array",
                                    "items": {"type": "integer", "minimum": 0},
                                    "minItems": 4,
                                    "maxItems": 4,
                                    "description": "[x, y, width, height]"
                                }
                            },
                            "required": ["name", "rect"]
                        }
                    }
                }),
                &["source", "output_dir", "regions"],
            ),
        },
    ]
}

pub fn tools() -> &'static [ToolSpec] {
    static TOOLS: OnceLock<Vec<ToolSpec>> = OnceLock::new();
    TOOLS.get_or_init(build_tools)
}

pub fn tool_by_name(name: &str) -> Option<&'static ToolSpec> {
    tools().iter().find(|tool| tool.name == name)
}

pub fn resource_list() -> &'static [McpResource] {
    RESOURCES
}

fn tools_json() -> Value {
    Value::Array(
        tools()
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "tier": tool.tier,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                })
            })
            .collect(),
    )
}

fn resources_json() -> Value {
    Value::Array(
        RESOURCES
            .iter()
            .map(|resource| {
                json!({
                    "uri": resource.uri,
                    "name": resource.name,
                    "mimeType": resource.mime_type,
                    "description": resource.description,
                })
            })
            .collect(),
    )
}

/// 快照门的权威输出（`ai_tool_surface.json` 的内容）。
pub fn surface_json() -> String {
    let value = json!({
        "version": 1,
        "transport": "mcp-stdio",
        "tools": tools_json(),
        "resources": resources_json(),
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("tool surface 序列化")
    )
}

/// 只含工具的稳定文本，供测试做集合断言。
pub fn resource_surface_json() -> String {
    serde_json::to_string_pretty(&resources_json()).expect("resource surface 序列化") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn ui_guide_example_passes_the_validator() {
        let block = UI_GUIDE
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("指南必须包含一个 json 代码块");
        let spec: rustlua_ui_spec::UiSpec =
            serde_json::from_str(block).expect("指南示例必须是合法 UiSpec JSON");
        let diagnostics =
            rustlua_ui_spec::validate_spec(&spec, &rustlua_ui_spec::Capabilities::core());
        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == rustlua_ui_spec::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "指南示例必须 0 error：{errors:#?}");
        // 契约里的枚举是 v_box/h_box：写成 vbox 会让客户端第一次调用就被 serde 拒绝。
        assert!(
            !UI_GUIDE.contains("`vbox`") && !UI_GUIDE.contains("`hbox`"),
            "指南不能用 vbox/hbox，必须写 v_box/h_box"
        );
    }

    #[test]
    fn tool_names_are_unique_and_structured() {
        let mut names = BTreeSet::new();
        for tool in tools() {
            assert!(names.insert(tool.name), "tool 名重复：{}", tool.name);
            assert_eq!(
                tool.input_schema["type"], "object",
                "{} 的 inputSchema 必须是 object",
                tool.name
            );
            assert!(
                tool.input_schema.get("additionalProperties").is_some(),
                "{} 必须显式声明 additionalProperties",
                tool.name
            );
        }
        assert_eq!(
            tools().iter().filter(|t| !t.tier.is_write()).count(),
            14,
            "只读 tool 数"
        );
        assert_eq!(tools().iter().filter(|t| t.tier.is_write()).count(), 31);
    }

    #[test]
    fn ui_tools_are_registered_with_expected_tiers() {
        for (name, tier) in [
            (ui::EXPORT_SPEC, ToolTier::Read),
            (ui::VALIDATE_SPEC, ToolTier::Read),
            (ui::PREVIEW_SPEC, ToolTier::Read),
            (ui::LIST_ASSETS, ToolTier::Read),
            (ui::APPLY_SPEC, ToolTier::T1),
            (ui::APPLY_PATCH, ToolTier::T1),
            (ui::CREATE_PRESET, ToolTier::T1),
            (ui::PROMOTE_ASSET, ToolTier::T2),
            (ui::GENERATE_ASSETS, ToolTier::T2),
            (ui::SLICE_ASSET, ToolTier::T2),
        ] {
            let spec = tool_by_name(name).unwrap_or_else(|| panic!("缺少 UI tool：{name}"));
            assert_eq!(spec.tier, tier, "{name} 的 tier");
        }
        for name in crate::ui_tools::TOOL_NAMES {
            assert!(tool_by_name(name).is_some(), "UI tool 未进 surface：{name}");
        }
    }

    /// 工具面快照门：**先红后刷**。改签名会先在这里变红；确认是有意变更后
    /// `AI_TOOL_SURFACE_BLESS=1 cargo test -p rustlua-editor-ai snapshot` 刷新基线。
    #[test]
    fn tool_surface_matches_baseline() {
        let baseline = include_str!("../ai_tool_surface.json");
        let current = surface_json();
        if std::env::var("AI_TOOL_SURFACE_BLESS").as_deref() == Ok("1") {
            std::fs::write(
                concat!(env!("CARGO_MANIFEST_DIR"), "/ai_tool_surface.json"),
                &current,
            )
            .expect("刷新 tool surface 基线");
            return;
        }
        assert_eq!(
            current, baseline,
            "AI tool 面与基线不一致。确认是有意增删/改签名后，跑\n\
             AI_TOOL_SURFACE_BLESS=1 cargo test -p rustlua-editor-ai tool_surface\n\
             刷新 ai_tool_surface.json 并随提交说明理由。"
        );
    }
}
