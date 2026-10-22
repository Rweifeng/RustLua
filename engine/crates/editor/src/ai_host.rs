//! M75：编辑器侧的 AI 能力层宿主。
//!
//! 所有 tool 都落在既有入口上：读 tool 直接查 `EditorApp` 的事实源；写 tool 一律发
//! `EditorCommand`（→ 事务/撤销），不在本文件里另写一套文档修改逻辑。

use crate::EditorApp;
use editor::{CommandResult, EditorCommand, ReparentPosition, SceneDocument, SelectionSource};
use rustlua_editor_ai::{AiBridge, ToolError, ToolHost, surface_tables, validate_api_calls};
use rustlua_scene::scene::{SceneEntity, SceneEntityId};
use rustlua_scripting::script::LuaRuntime;
use serde_json::{Map, Value, json};
use std::path::{Component, Path, PathBuf};

const LUA_API_SURFACE: &str =
    include_str!("../../rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json");

/// 一次 tool 调用的宿主视图。
pub(crate) struct EditorAiHost<'a> {
    pub app: &'a mut EditorApp,
}

impl EditorAiHost<'_> {
    fn document(&self) -> Result<&SceneDocument, ToolError> {
        self.app
            .document
            .current_scene
            .as_ref()
            .ok_or_else(|| ToolError::Rejected("当前没有打开的场景".to_owned()))
    }

    fn entity(&self, id: SceneEntityId) -> Result<&SceneEntity, ToolError> {
        self.document()?
            .scene
            .entities
            .get(&id)
            .ok_or_else(|| ToolError::Rejected(format!("实体不存在：{}", id.0)))
    }

    fn project_path(&self, raw: &str) -> Result<PathBuf, ToolError> {
        let path = Path::new(raw);
        if path.as_os_str().is_empty() {
            return Err(ToolError::InvalidArgs("路径不能为空".to_owned()));
        }
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(ToolError::InvalidArgs(format!(
                "只接受工程内相对路径（不得为绝对路径或含 ..）：{raw}"
            )));
        }
        Ok(self.app.project.root.join(path))
    }

    fn dispatch(&mut self, command: EditorCommand) -> Result<Value, ToolError> {
        let undo_before = self.app.history.undo_len();
        let result = self.app.dispatch(command);
        let undo_after = self.app.history.undo_len();
        match result {
            CommandResult::Applied => Ok(json!({
                "result": "applied",
                "undo_len": undo_after,
                "undo_entries_added": undo_after.saturating_sub(undo_before),
            })),
            CommandResult::NoOp => Ok(json!({"result": "noop", "undo_len": undo_after})),
            CommandResult::Rejected(reason) => Err(ToolError::Rejected(reason)),
        }
    }

    fn dispatch_edit(&mut self, command: EditorCommand) -> Result<Value, ToolError> {
        let value = self.dispatch(command)?;
        if value["result"] == "noop" {
            return Err(ToolError::Rejected(self.app.status.clone()));
        }
        Ok(value)
    }

    fn select_explicit(&mut self, ids: &[SceneEntityId]) -> Result<(), ToolError> {
        let Some(primary) = ids.first().copied() else {
            return Err(ToolError::InvalidArgs("实体列表不能为空".to_owned()));
        };
        for id in ids {
            self.entity(*id)?;
        }
        self.app
            .selection
            .set_many(ids.iter().copied(), Some(primary), SelectionSource::Command);
        Ok(())
    }

    fn scene_snapshot(&self) -> Value {
        let Some(document) = self.app.document.current_scene.as_ref() else {
            return json!({"scene": null});
        };
        let entities: Vec<Value> = document
            .scene
            .entities
            .values()
            .map(|entity| {
                let resources: std::collections::BTreeMap<String, String> = entity
                    .resources
                    .iter()
                    .map(|(key, id)| (key.clone(), id.to_string()))
                    .collect();
                json!({
                    "id": entity.id.0,
                    "name": entity.name,
                    "parent": entity.parent.map(|id| id.0),
                    "children": entity.children.iter().map(|id| id.0).collect::<Vec<_>>(),
                    "components": entity.components,
                    "resources": resources,
                })
            })
            .collect();
        json!({
            "name": document.scene.name,
            "dirty": document.dirty,
            "entities": entities,
        })
    }

    fn tree_node(&self, entity: &SceneEntity) -> Value {
        let document = self
            .app
            .document
            .current_scene
            .as_ref()
            .expect("tree_node 只在已打开场景时调用");
        let children: Vec<Value> = entity
            .children
            .iter()
            .filter_map(|id| document.scene.entities.get(id))
            .map(|child| self.tree_node(child))
            .collect();
        json!({
            "id": entity.id.0,
            "name": entity.name,
            "parent": entity.parent.map(|id| id.0),
            "components": entity.components.keys().collect::<Vec<_>>(),
            "children": children,
        })
    }

    fn read_tool(&mut self, name: &str, args: &Value) -> Result<Value, ToolError> {
        match name {
            "get_scene_tree" => {
                let document = self.document()?;
                let roots: Vec<Value> = document
                    .scene
                    .entities
                    .values()
                    .filter(|entity| entity.parent.is_none())
                    .map(|entity| self.tree_node(entity))
                    .collect();
                Ok(json!({
                    "scene": document.scene.name,
                    "dirty": document.dirty,
                    "path": document.path.as_ref().map(|path| {
                        path.as_path()
                            .strip_prefix(&self.app.project.root)
                            .unwrap_or(path.as_path())
                            .to_string_lossy()
                            .replace('\\', "/")
                    }),
                    "entities": roots,
                }))
            }
            "get_selection" => Ok(json!({
                "primary": self.app.selection.primary().map(|id| id.0),
                "entities": self.app.selection.ids().into_iter().map(|id| id.0).collect::<Vec<_>>(),
            })),
            "list_components" => {
                let groups: Vec<Value> = crate::panels::inspector::component_menu_groups()
                    .into_iter()
                    .map(|(category, names)| json!({"category": category, "components": names}))
                    .collect();
                Ok(json!({
                    "groups": groups,
                    "names": crate::panels::inspector::component_menu_groups()
                        .into_iter()
                        .flat_map(|(_, names)| names)
                        .collect::<Vec<_>>(),
                }))
            }
            "get_inspector_schema" => self.inspector_schema(args),
            "read_console" => {
                let limit = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(100)
                    .clamp(1, 500) as usize;
                let needle = args.get("contains").and_then(Value::as_str);
                let mut lines: Vec<String> = self
                    .app
                    .log_sink
                    .snapshot()
                    .into_iter()
                    .chain(
                        self.app
                            .console_notices
                            .iter()
                            .map(|line| format!("[editor] {line}")),
                    )
                    .filter(|line| needle.is_none_or(|needle| line.contains(needle)))
                    .collect();
                let total = lines.len();
                if lines.len() > limit {
                    lines = lines.split_off(total - limit);
                }
                Ok(json!({"lines": lines, "total_after_filter": total}))
            }
            "list_lua_api" => {
                let surface: Value = serde_json::from_str(LUA_API_SURFACE)
                    .map_err(|error| ToolError::Host(format!("Lua API 基线解析失败：{error}")))?;
                Ok(surface)
            }
            "get_lua_api" => self.lua_api_docs(args),
            "eval_lua_readonly" => {
                let expression = string_arg(args, "expression")?;
                let runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())
                    .map_err(|error| ToolError::Host(format!("创建 Lua VM 失败：{error}")))?;
                runtime
                    .set_instruction_budget(10_000_000)
                    .map_err(|error| ToolError::Rejected(error.to_string()))?;
                runtime
                    .eval_json(expression)
                    .map_err(|error| ToolError::Rejected(error.to_string()))
            }
            "capture_screenshot" => Err(ToolError::Unavailable(
                "capture_screenshot 需要真实窗口；请用 RUSTLUA_EDITOR_GUI_EVIDENCE 证据运行器"
                    .to_owned(),
            )),
            "get_project_config" => {
                let path = self.app.project.root.join("project.toml");
                let text = std::fs::read_to_string(&path).map_err(|error| {
                    ToolError::Rejected(format!("读取 {} 失败：{error}", path.display()))
                })?;
                Ok(json!({
                    "root": self.app.project.root.to_string_lossy().replace('\\', "/"),
                    "project_toml": text,
                }))
            }
            other => Err(ToolError::UnknownTool(other.to_owned())),
        }
    }

    fn inspector_schema(&self, args: &Value) -> Result<Value, ToolError> {
        let component_filter = args.get("component").and_then(Value::as_str);
        let registry = rustlua_ecs::component_schema::builtin_registry();
        let mut components = Vec::new();
        for schema in registry.values() {
            if component_filter.is_some_and(|name| name != schema.name) {
                continue;
            }
            let fields: Vec<Value> = schema
                .fields
                .iter()
                .map(|field| {
                    json!({
                        "name": field.name,
                        "kind": format!("{:?}", field.kind),
                        "required": field.required,
                        "choices": field.choices,
                        "rules": field.rules.iter().map(|rule| rule.describe()).collect::<Vec<_>>(),
                    })
                })
                .collect();
            components.push(json!({
                "name": schema.name,
                "version": schema.version,
                "fields": fields,
                "schema_driven": editor::schema_fields::is_schema_driven(schema.name),
            }));
        }
        let mut result = json!({"components": components});
        if let Some(entity_id) = args.get("entity").and_then(Value::as_u64) {
            let entity = self.entity(SceneEntityId(entity_id))?;
            let mut current = Map::new();
            for component in entity.components.keys() {
                let Some(rows) = editor::schema_fields::rows_for_component(entity, component)
                else {
                    continue;
                };
                let fields: Vec<Value> = rows
                    .iter()
                    .map(|row| {
                        json!({
                            "field": row.field,
                            "value": row.text,
                            "present": row.present,
                            "hint": row.hint,
                        })
                    })
                    .collect();
                current.insert(component.clone(), Value::Array(fields));
            }
            result["entity"] = json!({
                "id": entity.id.0,
                "name": entity.name,
                "raw_components": entity.components.keys().collect::<Vec<_>>(),
                "current": Value::Object(current),
            });
        }
        Ok(result)
    }

    fn lua_api_docs(&self, args: &Value) -> Result<Value, ToolError> {
        let surface: Value = serde_json::from_str(LUA_API_SURFACE)
            .map_err(|error| ToolError::Host(format!("Lua API 基线解析失败：{error}")))?;
        let module_filter = args.get("module").and_then(Value::as_str);
        let function_filter = args.get("function").and_then(Value::as_str);
        let mut tables = Vec::new();
        let mut docs = String::from("# RustLua Lua API\n\n");
        for table in surface["tables"].as_array().into_iter().flatten() {
            let Some(module) = table["name"].as_str() else {
                continue;
            };
            if module_filter.is_some_and(|filter| filter != module) {
                continue;
            }
            let funcs: Vec<&str> = table["funcs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|func| function_filter.is_none_or(|filter| filter == *func))
                .collect();
            if funcs.is_empty() {
                continue;
            }
            docs.push_str(&format!("## {module}\n\n"));
            for func in &funcs {
                docs.push_str(&format!("- `{module}.{func}`\n"));
            }
            docs.push('\n');
            tables.push(json!({"name": module, "funcs": funcs}));
        }
        Ok(json!({"tables": tables, "docs": docs}))
    }

    fn call_write(&mut self, name: &str, args: &Value) -> Result<Value, ToolError> {
        match name {
            "new_scene" => {
                let scene_name = string_arg(args, "name")?.to_owned();
                self.dispatch(EditorCommand::NewScene { name: scene_name })
            }
            "open_scene" => {
                let path = self.project_path(string_arg(args, "path")?)?;
                let result = self.dispatch(EditorCommand::OpenDocument { path })?;
                if self.app.pending_document_change.is_some() {
                    return Err(ToolError::Rejected(
                        "当前场景有未保存修改，打开操作已挂起等待确认".to_owned(),
                    ));
                }
                Ok(result)
            }
            "save_scene" => self.dispatch(EditorCommand::Save),
            "save_scene_as" => {
                let path = self.project_path(string_arg(args, "path")?)?;
                self.dispatch(EditorCommand::SaveAs { path })
            }
            "undo" => self.dispatch(EditorCommand::Undo),
            "redo" => self.dispatch(EditorCommand::Redo),
            "set_play_state" => {
                let action = string_arg(args, "action")?;
                let command = match action {
                    "start" => EditorCommand::StartPlay,
                    "stop" => EditorCommand::StopPlay,
                    "pause" => EditorCommand::PausePlay,
                    "resume" => EditorCommand::ResumePlay,
                    "restart" => EditorCommand::RestartPlay,
                    other => {
                        return Err(ToolError::InvalidArgs(format!("未知 play action：{other}")));
                    }
                };
                self.dispatch(command)
            }
            "step_play" => {
                let frames = args
                    .get("frames")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .clamp(1, 600) as u32;
                self.dispatch(EditorCommand::StepPlay { frames })
            }
            "create_entity" => self.create_entity(args),
            "select_entity" => {
                let id = entity_id_arg(args, "id")?;
                self.dispatch(EditorCommand::SelectEntity { id })
            }
            "rename_entity" => {
                let entity = entity_id_arg(args, "entity")?;
                let new_name = string_arg(args, "name")?.to_owned();
                self.dispatch(EditorCommand::RenameSelection {
                    entity,
                    name: new_name,
                })
            }
            "duplicate_entity" => {
                let explicit = args.get("entity").and_then(Value::as_u64);
                let before: Vec<SceneEntityId> =
                    self.document()?.scene.entities.keys().copied().collect();
                let mut value = if let Some(id) = explicit {
                    let id = SceneEntityId(id);
                    self.select_explicit(&[id])?;
                    self.dispatch_edit(EditorCommand::DuplicateSelection)?
                } else {
                    self.dispatch_edit(EditorCommand::DuplicateSelection)?
                };
                let new_ids: Vec<u64> = self
                    .document()?
                    .scene
                    .entities
                    .keys()
                    .filter(|id| !before.contains(id))
                    .map(|id| id.0)
                    .collect();
                value["new_entities"] = json!(new_ids);
                Ok(value)
            }
            "delete_entity" => {
                let ids = entity_ids_arg(args, "entities")?;
                self.select_explicit(&ids)?;
                let value = self.dispatch_edit(EditorCommand::DeleteSelection)?;
                Ok(json!({
                    "result": "applied",
                    "deleted": ids.iter().map(|id| id.0).collect::<Vec<_>>(),
                    "undo_len": value["undo_len"],
                }))
            }
            "reparent_entity" => {
                let entity = entity_id_arg(args, "entity")?;
                let target = args
                    .get("target")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        value.as_u64().map(SceneEntityId).ok_or_else(|| {
                            ToolError::InvalidArgs("target 必须是整数或 null".to_owned())
                        })
                    })
                    .transpose()?;
                let position = match args
                    .get("position")
                    .and_then(Value::as_str)
                    .unwrap_or("child")
                {
                    "before" => ReparentPosition::Before,
                    "after" => ReparentPosition::After,
                    "child" => ReparentPosition::Child,
                    other => {
                        return Err(ToolError::InvalidArgs(format!(
                            "未知 position：{other}（before/after/child）"
                        )));
                    }
                };
                self.select_explicit(&[entity])?;
                self.dispatch_edit(EditorCommand::ReparentSelection { target, position })
            }
            "add_component" => {
                let entity = entity_id_arg(args, "entity")?;
                let component = string_arg(args, "component")?.to_owned();
                self.dispatch(EditorCommand::AddComponent { entity, component })
            }
            "remove_component" => {
                let entity = entity_id_arg(args, "entity")?;
                let component = string_arg(args, "component")?.to_owned();
                self.dispatch(EditorCommand::RemoveComponent { entity, component })
            }
            "set_component_fields" => self.set_component_fields(args),
            "apply_prefab_override" => {
                let entity = entity_id_arg(args, "entity")?;
                let field = string_arg(args, "field")?.to_owned();
                self.dispatch(EditorCommand::ApplyPrefabOverride { entity, field })
            }
            "revert_prefab_override" => {
                let entity = entity_id_arg(args, "entity")?;
                let field = string_arg(args, "field")?.to_owned();
                self.dispatch(EditorCommand::RevertPrefabOverride { entity, field })
            }
            "revert_prefab_overrides" => {
                let entity = entity_id_arg(args, "entity")?;
                self.dispatch(EditorCommand::RevertPrefabOverrides { entity })
            }
            "reapply_prefab" => {
                let entity = entity_id_arg(args, "entity")?;
                self.dispatch(EditorCommand::ReapplyPrefab { entity })
            }
            "save_prefab_to_source" => {
                let entity = entity_id_arg(args, "entity")?;
                self.dispatch(EditorCommand::SavePrefabToSource { entity })
            }
            "unpack_prefab" => {
                let entity = entity_id_arg(args, "entity")?;
                self.dispatch(EditorCommand::UnpackPrefab { entity })
            }
            "write_lua_script" => self.write_lua_script(args),
            other => Err(ToolError::UnknownTool(other.to_owned())),
        }
    }

    fn create_entity(&mut self, args: &Value) -> Result<Value, ToolError> {
        let entity_name = args
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let parent = args
            .get("parent")
            .filter(|value| !value.is_null())
            .map(|value| {
                value
                    .as_u64()
                    .map(SceneEntityId)
                    .ok_or_else(|| ToolError::InvalidArgs("parent 必须是整数".to_owned()))
            })
            .transpose()?;
        let transform_3d = args
            .get("transform_3d")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let before: Vec<SceneEntityId> = self.document()?.scene.entities.keys().copied().collect();
        let value = self.dispatch(EditorCommand::CreateEntity {
            name: entity_name,
            parent,
            transform_3d,
        })?;
        let new_ids: Vec<u64> = self
            .document()?
            .scene
            .entities
            .keys()
            .filter(|id| !before.contains(id))
            .map(|id| id.0)
            .collect();
        let mut value = value;
        value["new_entities"] = json!(new_ids);
        Ok(value)
    }

    fn set_component_fields(&mut self, args: &Value) -> Result<Value, ToolError> {
        let component = string_arg(args, "component")?.to_owned();
        let fields_value = args
            .get("fields")
            .and_then(Value::as_object)
            .ok_or_else(|| ToolError::InvalidArgs("fields 必须是对象".to_owned()))?;
        let mut fields = Vec::with_capacity(fields_value.len());
        for (field, value) in fields_value {
            fields.push((field.clone(), field_text(value)?));
        }
        let ids = match args.get("entities") {
            Some(_) => entity_ids_arg(args, "entities")?,
            None => {
                let ids = self.app.selection.ids();
                if ids.is_empty() {
                    return Err(ToolError::InvalidArgs(
                        "未给 entities，且当前没有选中实体".to_owned(),
                    ));
                }
                ids
            }
        };
        // 先显式校验目标：批量命令会**静默跳过**没有该组件的实体，AI 拿到"成功"
        // 却少写了几个目标最难查。缺组件应当直接回错误，由 AI 先 add_component。
        {
            let document = self.document()?;
            let mut missing = Vec::new();
            for id in &ids {
                match document.scene.entities.get(id) {
                    None => {
                        return Err(ToolError::Rejected(format!("实体不存在：{}", id.0)));
                    }
                    Some(entity) if !entity.components.contains_key(&component) => {
                        missing.push(id.0);
                    }
                    Some(_) => {}
                }
            }
            if !missing.is_empty() {
                return Err(ToolError::Rejected(format!(
                    "实体 {:?} 没有组件 {component}，请先 add_component",
                    missing
                )));
            }
        }
        // 走多选批量命令：所有目标一次提交 = **一条** Undo（逐实体 dispatch 会留多条历史）。
        let previous = self.app.selection.ids();
        let previous_primary = self.app.selection.primary();
        self.select_explicit(&ids)?;
        let outcome = self.dispatch_edit(EditorCommand::SetComponentFieldsOnSelection {
            component: component.clone(),
            fields: fields.clone(),
        });
        self.app
            .selection
            .set_many(previous, previous_primary, SelectionSource::Command);
        let mut value = outcome?;
        value["entities"] = json!(ids.iter().map(|id| id.0).collect::<Vec<_>>());
        value["component"] = json!(component);
        value["fields"] = json!(fields.iter().map(|(field, _)| field).collect::<Vec<_>>());
        Ok(value)
    }

    fn write_lua_script(&mut self, args: &Value) -> Result<Value, ToolError> {
        let raw_path = string_arg(args, "path")?;
        let code = string_arg(args, "code")?;
        if Path::new(raw_path)
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("lua")
        {
            return Err(ToolError::InvalidArgs(
                "write_lua_script 只接受 .lua 文件".to_owned(),
            ));
        }
        let path = self.project_path(raw_path)?;
        let runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())
            .map_err(|error| ToolError::Host(format!("创建 Lua VM 失败：{error}")))?;
        runtime
            .validate_chunk(raw_path, code)
            .map_err(|error| ToolError::Rejected(format!("语法校验失败：{error}")))?;
        let surface: Value = serde_json::from_str(LUA_API_SURFACE)
            .map_err(|error| ToolError::Host(format!("Lua API 基线解析失败：{error}")))?;
        let issues = validate_api_calls(code, &surface_tables(&surface));
        if !issues.is_empty() {
            let details = issues
                .iter()
                .map(|issue| format!("第 {} 行：{}", issue.line, issue.message))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(ToolError::Rejected(format!(
                "Lua API 校验失败（修好后重试）：\n{details}"
            )));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                ToolError::Host(format!("创建目录 {} 失败：{error}", parent.display()))
            })?;
        }
        std::fs::write(&path, code)
            .map_err(|error| ToolError::Host(format!("写入 {} 失败：{error}", path.display())))?;
        Ok(json!({
            "result": "written",
            "path": raw_path,
            "bytes": code.len(),
        }))
    }
}

impl ToolHost for EditorAiHost<'_> {
    fn call_tool(&mut self, name: &str, args: &Value) -> Result<Value, ToolError> {
        match name {
            "get_scene_tree"
            | "get_selection"
            | "list_components"
            | "get_inspector_schema"
            | "read_console"
            | "list_lua_api"
            | "get_lua_api"
            | "eval_lua_readonly"
            | "capture_screenshot"
            | "get_project_config" => self.read_tool(name, args),
            _ => self.call_write(name, args),
        }
    }

    fn read_resource(&mut self, uri: &str) -> Result<String, ToolError> {
        match uri {
            "rustlua://lua-api/surface" => Ok(LUA_API_SURFACE.to_owned()),
            "rustlua://lua-api/docs" => {
                let value = self.lua_api_docs(&json!({}))?;
                Ok(value["docs"].as_str().unwrap_or_default().to_owned())
            }
            "rustlua://components" => self
                .read_tool("list_components", &json!({}))
                .map(|value| serde_json::to_string_pretty(&value).unwrap_or_default()),
            "rustlua://scene/schema" => {
                self.read_tool("get_inspector_schema", &json!({}))
                    .map(|value| {
                        serde_json::to_string_pretty(&value["components"]).unwrap_or_default()
                    })
            }
            "rustlua://project/config" => {
                let path = self.app.project.root.join("project.toml");
                std::fs::read_to_string(&path).map_err(|error| {
                    ToolError::Rejected(format!("读取 {} 失败：{error}", path.display()))
                })
            }
            other => Err(ToolError::UnknownTool(format!("resource {other}"))),
        }
    }

    fn snapshot(&mut self) -> Option<Value> {
        Some(self.scene_snapshot())
    }
}

fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError::InvalidArgs(format!("缺少字符串参数 `{key}`")))
}

fn entity_id_arg(args: &Value, key: &str) -> Result<SceneEntityId, ToolError> {
    args.get(key)
        .and_then(Value::as_u64)
        .map(SceneEntityId)
        .ok_or_else(|| ToolError::InvalidArgs(format!("缺少整数参数 `{key}`")))
}

fn entity_ids_arg(args: &Value, key: &str) -> Result<Vec<SceneEntityId>, ToolError> {
    let values = args
        .get(key)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .ok_or_else(|| ToolError::InvalidArgs(format!("缺少非空数组参数 `{key}`")))?;
    values
        .iter()
        .map(|value| {
            value
                .as_u64()
                .map(SceneEntityId)
                .ok_or_else(|| ToolError::InvalidArgs(format!("`{key}` 只能包含整数")))
        })
        .collect()
}

fn field_text(value: &Value) -> Result<String, ToolError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        _ => Err(ToolError::InvalidArgs(
            "字段值只支持字符串/数字/布尔".to_owned(),
        )),
    }
}

impl EditorApp {
    /// `--mcp-stdio` / `RUSTLUA_EDITOR_MCP=1` 时启动协议线程。
    pub(crate) fn start_ai_bridge(
        &mut self,
        context: &eframe::egui::Context,
    ) -> Result<(), String> {
        let context = context.clone();
        self.ai = AiBridge::start_stdio(move || context.request_repaint())
            .map_err(|error| format!("启动 MCP server 失败：{error}"))?;
        eprintln!("[mcp] stdio server 已启动（tool 面见 rustlua://lua-api/surface）");
        Ok(())
    }

    /// 主线程泵：协议线程的 tool 调用在这里落到 `EditorCommand`。
    pub(crate) fn pump_ai(&mut self, context: &eframe::egui::Context) {
        let policy = self.ai_policy;
        let mut bridge = std::mem::replace(&mut self.ai, AiBridge::detached());
        {
            let mut host = EditorAiHost { app: self };
            bridge.pump(&mut host, policy);
        }
        let has_pending = bridge.has_pending();
        self.ai = bridge;
        if has_pending {
            context.request_repaint();
        }
    }

    #[cfg(test)]
    pub(crate) fn ai_snapshot_for_test(&mut self) -> Value {
        let mut host = EditorAiHost { app: self };
        host.snapshot().unwrap_or(Value::Null)
    }

    /// 确认窗：T2/T3（以及未勾选自动批处理的 T1）必须在这里显式放行。
    pub(crate) fn draw_ai_gate_window(&mut self, context: &eframe::egui::Context) {
        if !self.ai.has_pending() {
            return;
        }
        let calls: Vec<(String, Value, &'static str)> = self
            .ai
            .pending()
            .iter()
            .enumerate()
            .map(|(index, call)| {
                (
                    format!("{index}. {}", call.name),
                    call.args.clone(),
                    rustlua_editor_ai::bridge::tier_label(call.tier),
                )
            })
            .collect();
        let mut decisions: Vec<(usize, bool)> = Vec::new();
        let mut auto_t1 = self.ai_policy.auto_t1;
        eframe::egui::Window::new("AI 动作确认")
            .collapsible(false)
            .resizable(true)
            .show(context, |ui| {
                ui.label("外部 AI 客户端请求执行写动作。拒绝不会改动任何文档。");
                ui.checkbox(&mut auto_t1, "本次会话自动批准 T1（场景内可撤销写）");
                for (index, (label, args, tier)) in calls.iter().enumerate() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.strong(label);
                            ui.label(format!("[{tier}]"));
                        });
                        ui.label(serde_json::to_string(args).unwrap_or_default());
                        ui.horizontal(|ui| {
                            if ui.button("批准").clicked() {
                                decisions.push((index, true));
                            }
                            if ui.button("拒绝").clicked() {
                                decisions.push((index, false));
                            }
                        });
                    });
                }
            });
        self.ai_policy.auto_t1 = auto_t1;
        // 从后往前删，避免前面的批准/拒绝把后面的下标挤位。
        decisions.sort_by_key(|(index, _)| std::cmp::Reverse(*index));
        for (index, approve) in decisions {
            let mut bridge = std::mem::replace(&mut self.ai, AiBridge::detached());
            if approve {
                let mut host = EditorAiHost { app: self };
                bridge.approve(index, &mut host);
            } else {
                bridge.reject(index, "用户在编辑器内拒绝");
            }
            self.ai = bridge;
            if self.ai.has_pending() {
                context.request_repaint();
            }
        }
        context.request_repaint();
    }
}

/// 供内置面板 / agent / 测试直接调 tool（不经协议线程）。
#[cfg(test)]
pub(crate) fn call_tool_direct(
    app: &mut EditorApp,
    policy: rustlua_editor_ai::GatePolicy,
    name: &str,
    args: Value,
) -> Result<Value, ToolError> {
    let mut host = EditorAiHost { app };
    rustlua_editor_ai::execute_tool(&mut host, policy, name, &args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::ProjectInfo;
    use rustlua_editor_ai::{GatePolicy, ReplayFixture, run_replay};
    use rustlua_infra::logging::LogSink;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-editor-ai-{label}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    fn test_app(label: &str) -> (EditorApp, PathBuf) {
        let root = temp_root(label);
        let mut app = EditorApp::new(
            ProjectInfo {
                root: root.clone(),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        );
        app.project_loaded = true;
        let mut document = SceneDocument::new_scene("main");
        document
            .scene
            .add_entity(SceneEntityId(1), "Root", None)
            .unwrap();
        app.document.current_scene = Some(document);
        (app, root)
    }

    fn direct(app: &mut EditorApp, name: &str, args: Value) -> Result<Value, ToolError> {
        call_tool_direct(app, GatePolicy::HEADLESS, name, args)
    }

    #[test]
    fn create_component_fields_are_one_undo_and_undo_restores() {
        let (mut app, _root) = test_app("undo");
        let created =
            direct(&mut app, "create_entity", json!({"name": "AI Entity"})).expect("create");
        let id = created["new_entities"][0].as_u64().expect("id");
        direct(
            &mut app,
            "add_component",
            json!({"entity": id, "component": "Transform"}),
        )
        .expect("add component");
        let before_fields = app.document.current_scene.as_ref().unwrap().clone();
        let result = direct(
            &mut app,
            "set_component_fields",
            json!({
                "entities": [id],
                "component": "Transform",
                "fields": {"position": "4,5,0"}
            }),
        )
        .expect("set fields");
        assert_eq!(result["undo_entries_added"], 1);
        let position = &app.document.current_scene.as_ref().unwrap().scene.entities
            [&SceneEntityId(id)]
            .components["Transform"];
        assert!(
            position.contains("position=4.000000,5.000000,0.000000"),
            "实际组件串：{position}"
        );
        direct(&mut app, "undo", json!({})).expect("undo");
        let after = app.document.current_scene.as_ref().unwrap();
        let restored = before_fields
            .scene
            .entities
            .get(&SceneEntityId(id))
            .map(|entity| entity.components.clone())
            .unwrap();
        assert_eq!(
            after.scene.entities[&SceneEntityId(id)].components,
            restored,
            "undo 必须完整收回字段写入"
        );
    }

    #[test]
    fn delete_entity_uses_explicit_ids() {
        let (mut app, _root) = test_app("delete");
        let a = direct(&mut app, "create_entity", json!({"name": "A"})).unwrap()["new_entities"][0]
            .as_u64()
            .unwrap();
        let b = direct(&mut app, "create_entity", json!({"name": "B"})).unwrap()["new_entities"][0]
            .as_u64()
            .unwrap();
        let policy = GatePolicy {
            allow_t2: true,
            ..GatePolicy::HEADLESS
        };
        call_tool_direct(&mut app, policy, "delete_entity", json!({"entities": [a]}))
            .expect("delete");
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        assert!(!scene.entities.contains_key(&SceneEntityId(a)));
        assert!(scene.entities.contains_key(&SceneEntityId(b)));
    }

    #[test]
    fn t2_tool_is_rejected_by_headless_policy_before_host() {
        let (mut app, _root) = test_app("gate");
        let error = direct(&mut app, "delete_entity", json!({"entities": [1]}))
            .expect_err("T2 必须被确认门拦住");
        assert!(matches!(error, ToolError::NeedsConfirmation { .. }));
        assert!(
            app.document
                .current_scene
                .as_ref()
                .unwrap()
                .scene
                .entities
                .contains_key(&SceneEntityId(1))
        );
    }

    #[test]
    fn t3_session_tools_are_rejected_headless_without_hanging() {
        let (mut app, _root) = test_app("gate-t3");
        let dirty_before = app.document.current_scene.as_ref().expect("测试场景").dirty;
        for (name, args) in [
            ("new_scene", json!({"name": "other"})),
            ("open_scene", json!({"path": "scenes/other.scene"})),
            ("save_scene", json!({})),
            ("set_play_state", json!({"action": "start"})),
        ] {
            let error =
                direct(&mut app, name, args).expect_err("T3 必须在无确认通道时直接拒绝（不挂起）");
            assert!(
                matches!(error, ToolError::NeedsConfirmation { .. }),
                "{name} 应报 NeedsConfirmation，实际：{error}"
            );
        }
        let scene = app
            .document
            .current_scene
            .as_ref()
            .expect("拒绝后原场景必须还在");
        assert_eq!(scene.scene.name, "main", "拒绝不得改动文档");
        assert_eq!(scene.dirty, dirty_before, "拒绝不得改变文档的脏状态");
    }

    #[test]
    fn lua_repair_loop_rejects_then_accepts_after_fix() {
        let (mut app, root) = test_app("lua-repair");
        let policy = GatePolicy {
            allow_t2: true,
            ..GatePolicy::HEADLESS
        };
        let path = "scripts/repair.lua";
        let bad = call_tool_direct(
            &mut app,
            policy,
            "write_lua_script",
            json!({"path": path, "code": "entity.spwan(\"x\")"}),
        )
        .expect_err("第一轮必须判红");
        assert!(bad.message().contains("entity.spawn"), "错误里要带可修候选");
        assert!(!root.join(path).exists(), "判红时不得落盘");
        // 模拟 AI 按错误信息修好后再提交。
        call_tool_direct(
            &mut app,
            policy,
            "write_lua_script",
            json!({"path": path, "code": "entity.spawn(\"x\")"}),
        )
        .expect("修好后应落盘");
        assert!(root.join(path).is_file());
    }

    #[test]
    fn write_lua_script_rejects_hallucinated_api_and_keeps_disk_clean() {
        let (mut app, root) = test_app("lua-bad");
        let policy = GatePolicy {
            allow_t2: true,
            ..GatePolicy::HEADLESS
        };
        let error = call_tool_direct(
            &mut app,
            policy,
            "write_lua_script",
            json!({"path": "scripts/bad.lua", "code": "entity.spwan(\"x\")"}),
        )
        .expect_err("幻觉 API 必须被拒");
        let message = error.message();
        assert!(message.contains("entity.spwan"), "{message}");
        assert!(message.contains("entity.spawn"), "应给出候选：{message}");
        assert!(!root.join("scripts/bad.lua").exists(), "校验失败不得落盘");
    }

    #[test]
    fn write_lua_script_writes_valid_script() {
        let (mut app, root) = test_app("lua-good");
        let policy = GatePolicy {
            allow_t2: true,
            ..GatePolicy::HEADLESS
        };
        call_tool_direct(
            &mut app,
            policy,
            "write_lua_script",
            json!({"path": "scripts/good.lua", "code": "local x = 1\nentity.spawn(\"x\")\nreturn x"}),
        )
        .expect("合法脚本应落盘");
        assert!(root.join("scripts/good.lua").is_file());
    }

    #[test]
    fn read_tools_return_live_state() {
        let (mut app, _root) = test_app("read");
        let id =
            direct(&mut app, "create_entity", json!({"name": "Live"})).unwrap()["new_entities"][0]
                .as_u64()
                .unwrap();
        let tree = direct(&mut app, "get_scene_tree", json!({})).unwrap();
        assert_eq!(tree["scene"], "main");
        assert!(tree.to_string().contains("Live"));

        direct(&mut app, "select_entity", json!({"id": id})).unwrap();
        let selection = direct(&mut app, "get_selection", json!({})).unwrap();
        assert_eq!(selection["primary"], id);

        let components = direct(&mut app, "list_components", json!({})).unwrap();
        assert!(components["names"].to_string().contains("Transform"));

        let api = direct(&mut app, "list_lua_api", json!({})).unwrap();
        assert!(
            api["tables"]
                .as_array()
                .is_some_and(|tables| !tables.is_empty())
        );
    }

    #[test]
    fn replay_fixture_is_deterministic_across_fresh_sessions() {
        let fixture: ReplayFixture = serde_json::from_str(include_str!(
            "../tests/fixtures/ai_replay_entity_lifecycle.json"
        ))
        .expect("fixture");
        let (mut first, _a) = test_app("replay-a");
        let (mut second, _b) = test_app("replay-b");
        let report_a = {
            let mut host = EditorAiHost { app: &mut first };
            run_replay(&fixture, &mut host).expect("replay a")
        };
        let report_b = {
            let mut host = EditorAiHost { app: &mut second };
            run_replay(&fixture, &mut host).expect("replay b")
        };
        assert!(report_a.ok() && report_b.ok());
        assert_eq!(
            first.document.current_scene.as_ref().unwrap().scene,
            second.document.current_scene.as_ref().unwrap().scene,
            "同一 fixture 两次回放必须得到同一场景"
        );
        assert_eq!(first.ai_snapshot_for_test(), second.ai_snapshot_for_test());
    }

    #[test]
    fn tool_surface_covers_editor_host_write_paths() {
        // 宿主 match 必须认识 tool 面里的每个写 tool（防止快照里出现无人实现的 tool）。
        let (mut app, _root) = test_app("surface");
        for tool in rustlua_editor_ai::tools()
            .iter()
            .filter(|tool| tool.tier.is_write())
        {
            if tool.name == "write_lua_script" {
                continue;
            }
            // 只验证分发能到达宿主（参数错误 / no-op 也算到达），不会命中 UnknownTool。
            let outcome = direct(&mut app, tool.name, json!({}));
            if let Err(error) = outcome {
                assert!(
                    !matches!(error, ToolError::UnknownTool(_)),
                    "{} 在宿主里没有实现",
                    tool.name
                );
            }
        }
    }
}
