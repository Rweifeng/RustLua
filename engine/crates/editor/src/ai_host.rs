//! M75：编辑器侧的 AI 能力层宿主。
//!
//! 所有 tool 都落在既有入口上：读 tool 直接查 `EditorApp` 的事实源；写 tool 一律发
//! `EditorCommand`（→ 事务/撤销），不在本文件里另写一套文档修改逻辑。

use crate::EditorApp;
use editor::ui_spec_host::{
    UiHostError, UiHostErrorCode, UiSpecStore, base64_encode, sanitize_asset_key,
};
use editor::{CommandResult, EditorCommand, ReparentPosition, SceneDocument, SelectionSource};
use rustlua_editor_ai::ui_tools::{
    ApplyPatchRequest, ApplyResponse, ApplySpecRequest, CreatePresetRequest, CreatePresetResponse,
    ExportSpecRequest, ExportSpecResponse, GenerateAssetsRequest, GenerateAssetsResponse,
    GeneratedAsset, ListAssetsRequest, ListAssetsResponse, PreviewSpecRequest, PromoteAssetRequest,
    PromoteAssetResponse, SliceAssetRequest, SliceAssetResponse, SlicedAssetEntry, UiAssetEntry,
    ValidateSpecRequest, ValidateSpecResponse,
};
use rustlua_editor_ai::{
    AiBridge, ImageProvider, ImageRequest, LocalImageProvider, RemoteImageConfig,
    RemoteImageProvider, ToolError, ToolHost, surface_tables, validate_api_calls,
};
use rustlua_scene::scene::{SceneEntity, SceneEntityId};
use rustlua_scripting::script::LuaRuntime;
use rustlua_ui_spec::{AssetProvenance, AssetSource, Severity, content_hash};
use serde::de::DeserializeOwned;
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
            "read_resource" => {
                let uri = string_arg(args, "uri")?;
                self.read_resource(uri).map(Value::String)
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

    /// M76-03：`ui_*` 工具的唯一执行体。写动作只生成提案，接受入口留给编辑器
    /// （UI Studio / 确认流），不会绕过 M75 的 tier 确认门。
    fn ui_tool(&mut self, name: &str, args: &Value) -> Result<Value, ToolError> {
        use rustlua_editor_ai::ui_tools::names as ui;
        let store = UiSpecStore::new(self.app.project.root.clone());
        match name {
            ui::EXPORT_SPEC => {
                let request: ExportSpecRequest = parse_ui_request(args)?;
                let document = store.load(&request.document).map_err(ui_tool_error)?;
                to_json(ExportSpecResponse {
                    document: document.document,
                    revision: document.revision,
                    spec: document.spec,
                })
            }
            ui::VALIDATE_SPEC => {
                let request: ValidateSpecRequest = parse_ui_request(args)?;
                let diagnostics = match store.validate(&request.spec) {
                    Ok(diagnostics) => diagnostics,
                    Err(error) => error.diagnostics,
                };
                to_json(ValidateSpecResponse { diagnostics })
            }
            ui::PREVIEW_SPEC => {
                let request: PreviewSpecRequest = parse_ui_request(args)?;
                let png = store
                    .preview(&request.spec, request.width, request.height, request.scale)
                    .map_err(ui_tool_error)?;
                Ok(json!({
                    "width": request.width,
                    "height": request.height,
                    "bytes": png.len(),
                    "png_base64": base64_encode(&png),
                }))
            }
            ui::LIST_ASSETS => {
                let request: ListAssetsRequest = parse_ui_request(args)?;
                let assets = store
                    .list_assets(request.document.as_deref())
                    .map_err(ui_tool_error)?
                    .into_iter()
                    .map(|listing| UiAssetEntry {
                        key: listing.key,
                        role: listing.role,
                        path: listing.path,
                        provenance: listing.provenance,
                        bytes: listing.bytes,
                    })
                    .collect();
                to_json(ListAssetsResponse { assets })
            }
            ui::APPLY_SPEC => {
                let request: ApplySpecRequest = parse_ui_request(args)?;
                let proposal = store
                    .submit_spec(&request.document, request.base_revision, request.spec)
                    .map_err(ui_tool_error)?;
                to_json(ApplyResponse { proposal })
            }
            ui::APPLY_PATCH => {
                let request: ApplyPatchRequest = parse_ui_request(args)?;
                let proposal = store
                    .submit_patch(&request.document, request.patch)
                    .map_err(ui_tool_error)?;
                to_json(ApplyResponse { proposal })
            }
            ui::CREATE_PRESET => {
                let request: CreatePresetRequest = parse_ui_request(args)?;
                // 只新建：已存在时绝不覆盖，避免「一句话」把别人正在编辑的文档冲掉。
                let path = store.resolve(&request.document).map_err(ui_tool_error)?;
                if path.exists() {
                    return Err(ToolError::Rejected(format!(
                        "UI 预设已存在：{}；先 ui_export_spec 取 revision，再用 ui_apply_patch 修改",
                        request.document
                    )));
                }
                let diagnostics = store.validate(&request.spec).map_err(ui_tool_error)?;
                // 走与 UI Studio 相同的提案 → 接受路径；区别只是新文档的 base_revision 固定为 0。
                let proposal = store
                    .submit_spec(&request.document, 0, request.spec.clone())
                    .map_err(ui_tool_error)?;
                let accepted = store.accept(&proposal.proposal_id).map_err(ui_tool_error)?;
                let preview_png_base64 = if request.preview {
                    let png = store
                        .preview(&accepted.spec, request.width, request.height, request.scale)
                        .map_err(ui_tool_error)?;
                    Some(base64_encode(&png))
                } else {
                    None
                };
                to_json(CreatePresetResponse {
                    document: accepted.document,
                    revision: accepted.revision,
                    spec: accepted.spec,
                    diagnostics,
                    preview_png_base64,
                    width: request.width,
                    height: request.height,
                })
            }
            ui::PROMOTE_ASSET => {
                let request: PromoteAssetRequest = parse_ui_request(args)?;
                let provenance = store
                    .promote_asset(
                        &request.asset_id,
                        request.reviewed,
                        request.license,
                        request.used_by,
                    )
                    .map_err(ui_tool_error)?;
                to_json(PromoteAssetResponse { provenance })
            }
            ui::GENERATE_ASSETS => {
                let request: GenerateAssetsRequest = parse_ui_request(args)?;
                let max_images = request.max_images.unwrap_or(4).max(1);
                if request.plan.len() as u32 > max_images {
                    return Err(ToolError::Rejected(format!(
                        "单次最多生成 {max_images} 张，本次计划 {} 张",
                        request.plan.len()
                    )));
                }
                // 默认走本地确定性 provider：CI/无密钥环境也能跑完整链路。远程要显式开启，
                // 密钥只从环境变量读（见 `rustlua_editor_ai::image_provider`）。
                let provider: Box<dyn ImageProvider> =
                    match std::env::var("RUSTLUA_AI_IMAGE_PROVIDER").as_deref() {
                        Ok("remote") => {
                            Box::new(RemoteImageProvider::new(RemoteImageConfig::from_env()))
                        }
                        _ => Box::new(LocalImageProvider::new()),
                    };
                let model = std::env::var("RUSTLUA_AI_IMAGE_MODEL")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "local-checker".to_owned());
                let mut generated = Vec::new();
                let mut diagnostics = Vec::new();
                for plan in &request.plan {
                    let plan_diagnostics = plan.validate();
                    let errors: Vec<String> = plan_diagnostics
                        .iter()
                        .filter(|diagnostic| diagnostic.severity == Severity::Error)
                        .map(|diagnostic| diagnostic.message.clone())
                        .collect();
                    diagnostics.extend(plan_diagnostics);
                    if !errors.is_empty() {
                        return Err(ToolError::Rejected(format!(
                            "资产计划 {} 不合法：{}",
                            plan.key,
                            errors.join("；")
                        )));
                    }
                    let Some(generate) = plan.generate.clone() else {
                        return Err(ToolError::Rejected(format!(
                            "资产 {} 没有 generate 计划（已有 path 的资产无需生成）",
                            plan.key
                        )));
                    };
                    let image_request = ImageRequest {
                        prompt: generate.prompt.clone(),
                        size: generate.size.unwrap_or([64, 64]),
                        negative: generate.negative.clone(),
                        seed: generate.seed,
                        style_tokens: Default::default(),
                    };
                    let output = provider.generate(&image_request).map_err(|failure| {
                        ToolError::Rejected(format!(
                            "出图失败（{}）：{}",
                            failure.code, failure.message
                        ))
                    })?;
                    let path = plan.path.clone().unwrap_or_else(|| {
                        format!("assets/generated/ui/{}.png", sanitize_asset_key(&plan.key))
                    });
                    store
                        .write_asset(&path, &output.png)
                        .map_err(ui_tool_error)?;
                    // 生成资产一律先记为「未审阅」：进入发布包前必须人工 promote。
                    let provenance = AssetProvenance {
                        asset_id: plan.key.clone(),
                        source: AssetSource::Generated,
                        provider: Some(provider.name().to_owned()),
                        model: Some(model.clone()),
                        prompt_hash: Some(format!(
                            "{:016x}",
                            content_hash(generate.prompt.as_bytes())
                        )),
                        seed: generate.seed,
                        license: None,
                        reviewed: false,
                        used_by: Vec::new(),
                        created_at: None,
                    };
                    store
                        .register_provenance(&provenance)
                        .map_err(ui_tool_error)?;
                    generated.push(GeneratedAsset {
                        key: plan.key.clone(),
                        png: output.png,
                        generation: output.generation,
                        provenance,
                    });
                }
                to_json(GenerateAssetsResponse {
                    generated,
                    diagnostics,
                })
            }
            ui::SLICE_ASSET => {
                let request: SliceAssetRequest = parse_ui_request(args)?;
                let regions = request
                    .regions
                    .iter()
                    .map(|region| {
                        rustlua_ui_spec::SliceRegion::new(region.name.clone(), region.rect)
                    })
                    .collect();
                let manifest = store
                    .slice_asset(&request.source, regions, &request.output_dir)
                    .map_err(ui_tool_error)?;
                to_json(SliceAssetResponse {
                    source: manifest.source,
                    source_width: manifest.source_width,
                    source_height: manifest.source_height,
                    output_dir: manifest.output_dir,
                    slices: manifest
                        .slices
                        .into_iter()
                        .map(|slice| SlicedAssetEntry {
                            name: slice.name,
                            path: slice.path,
                            rect: slice.rect,
                            bytes: slice.bytes,
                        })
                        .collect(),
                })
            }
            other => Err(ToolError::UnknownTool(other.to_owned())),
        }
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
            "open_project" => self.open_project(args),
            other => Err(ToolError::UnknownTool(other.to_owned())),
        }
    }

    /// 工程切换：复用「文件 > 打开项目」那条路（`PendingChange::SwitchProject` →
    /// 后台导入），不在这里另写一套工程加载。
    ///
    /// 导入是**异步**的，真正的文档替换发生在 `poll_progress` 落地那一帧，所以本函数
    /// 只能如实回「已排队」——阻塞等会冻住主循环，进度反而永远不落地。
    fn open_project(&mut self, args: &Value) -> Result<Value, ToolError> {
        let raw = string_arg(args, "path")?;
        let requested = {
            let path = Path::new(raw);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                self.app.project.root.join(path)
            }
        };
        let target = editor::open_project(&requested)
            .map_err(|error| ToolError::Rejected(error.to_string()))?;
        if !target.root.join("project.toml").is_file() {
            return Err(ToolError::Rejected(format!(
                "不是独立游戏项目：缺少 project.toml（{}）",
                crate::util::display_path(&target.root)
            )));
        }
        if crate::util::display_path(&target.root)
            == crate::util::display_path(&self.app.project.root)
        {
            return Err(ToolError::Rejected(format!(
                "当前已打开该工程：{}",
                crate::util::display_path(&target.root)
            )));
        }
        // `start_import` 在后台任务在跑时静默 return，切过去只会谎报成功。
        if self.app.progress.running {
            return Err(ToolError::Rejected(
                "已有后台任务在跑（导入/发布），等它结束后再切工程".to_owned(),
            ));
        }
        if self.app.pending_document_change.is_some() {
            return Err(ToolError::Rejected(
                "已有一条待确认的文档操作，先在编辑器里裁决它".to_owned(),
            ));
        }
        self.app
            .request_document_change(crate::actions::PendingChange::SwitchProject {
                path: target.root.to_string_lossy().into_owned(),
            });
        if self.app.pending_document_change.is_some() {
            return Err(ToolError::Rejected(format!(
                "当前场景有未保存修改，切到 {} 已挂起等待编辑器内确认",
                crate::util::display_path(&target.root)
            )));
        }
        Ok(json!({
            "result": "queued",
            "target_root": crate::util::display_path(&target.root),
            "note": "后台导入完成后才真正替换文档；用 get_project_config 轮询 root 确认落地",
        }))
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
            | "get_project_config"
            | "read_resource" => self.read_tool(name, args),
            name if rustlua_editor_ai::ui_tools::is_ui_tool(name) => self.ui_tool(name, args),
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
            "rustlua://ui/guide" => Ok(rustlua_editor_ai::UI_GUIDE.to_owned()),
            other => Err(ToolError::UnknownTool(format!("未知资源：{other}"))),
        }
    }

    fn snapshot(&mut self) -> Option<Value> {
        Some(self.scene_snapshot())
    }
}

fn parse_ui_request<T: DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    serde_json::from_value(args.clone())
        .map_err(|error| ToolError::InvalidArgs(format!("参数不符合 UI tool 契约：{error}")))
}

fn ui_tool_error(error: UiHostError) -> ToolError {
    let mut message = error.message.clone();
    for diagnostic in &error.diagnostics {
        if let Some(fix) = &diagnostic.fix {
            message.push_str(&format!(
                "\n[{}] {}：{}（建议：{}）",
                diagnostic.code, diagnostic.path, diagnostic.message, fix
            ));
        }
    }
    match error.code {
        UiHostErrorCode::InvalidArgument => ToolError::InvalidArgs(message),
        UiHostErrorCode::Unavailable => ToolError::Unavailable(message),
        UiHostErrorCode::Io => ToolError::Host(message),
        UiHostErrorCode::StaleRevision
        | UiHostErrorCode::InvalidSpec
        | UiHostErrorCode::Capability
        | UiHostErrorCode::NotFound
        | UiHostErrorCode::ConfirmationRequired
        | UiHostErrorCode::BudgetExceeded => ToolError::Rejected(message),
    }
}

fn to_json<T: serde::Serialize>(value: T) -> Result<Value, ToolError> {
    serde_json::to_value(value).map_err(|error| ToolError::Host(format!("序列化响应失败：{error}")))
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
        eprintln!(
            "[mcp] stdio server 已启动（tool 面见 rustlua://lua-api/surface）· gate auto_t1={} allow_t2={} allow_t3={} queue={}",
            self.ai_policy.auto_t1,
            self.ai_policy.allow_t2,
            self.ai_policy.allow_t3,
            self.ai_policy.queue_unconfirmed
        );
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
            ("open_project", json!({"path": "."})),
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

    /// `open_project` 是 T3：显式放行确认门才进得了宿主侧的前置校验。
    fn allow_t3(app: &mut EditorApp, args: Value) -> Result<Value, ToolError> {
        let policy = GatePolicy {
            allow_t3: true,
            ..GatePolicy::HEADLESS
        };
        call_tool_direct(app, policy, "open_project", args)
    }

    /// 用真实样例工程起一个 app：`open_project` 的相对路径要拿真 root 当基准才有意义。
    fn sample_app(example: &str) -> EditorApp {
        let project =
            editor::open_project(rustlua_infra::games_dir().join("examples").join(example))
                .expect("样例工程应能打开");
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.project_loaded = true;
        let mut document = SceneDocument::new_scene("main");
        document
            .scene
            .add_entity(SceneEntityId(1), "Root", None)
            .unwrap();
        // 造实体把文档标脏了：这条测试要测的是「clean 时直接排队」，脏文档另有其一。
        document.dirty = false;
        app.document.current_scene = Some(document);
        app
    }

    fn examples_root() -> PathBuf {
        rustlua_infra::games_dir().join("examples")
    }

    fn ends_with_game_api(path: &str) -> bool {
        path.replace('\\', "/").ends_with("examples/game_api")
    }

    #[test]
    fn open_project_rejects_a_directory_that_is_not_a_project() {
        let (mut app, _root) = test_app("open-project-bad");
        let foreign = temp_root("open-project-foreign");
        let error = allow_t3(&mut app, json!({"path": foreign.to_string_lossy()}))
            .expect_err("空目录不是工程，必须在排队前拒绝");
        assert!(error.message().contains("必需目录"), "{}", error.message());
        assert!(!app.progress.running, "拒绝时不得启动后台导入");
        let _ = std::fs::remove_dir_all(foreign);
    }

    #[test]
    fn open_project_rejects_the_project_that_is_already_open() {
        let mut app = sample_app("game_api");
        let same = examples_root().join("game_api");
        let error = allow_t3(&mut app, json!({"path": same.to_string_lossy()}))
            .expect_err("切到当前工程不该重载一遍自己");
        assert!(
            error.message().contains("当前已打开"),
            "{}",
            error.message()
        );
        assert!(!app.progress.running);
    }

    #[test]
    fn open_project_queues_the_background_import_for_another_project() {
        let mut app = sample_app("game_jhyt");
        // 相对当前工程根、带 `..` 的写法是这条连接器的实际调用形状。
        let value = allow_t3(&mut app, json!({"path": "../game_api"})).expect("应排队成功");
        assert_eq!(value["result"], "queued");
        assert!(
            value["target_root"]
                .as_str()
                .is_some_and(ends_with_game_api),
            "读数要如实说出目标：{value}"
        );
        assert!(
            ends_with_game_api(&app.import_path),
            "后台导入的目标错位：{}",
            app.import_path
        );
        assert!(app.progress.running, "必须真的把后台导入排上");
        assert!(
            app.project.root.to_string_lossy().contains("game_jhyt"),
            "导入是异步的，此刻工程根还不能已经换掉（换了就是读数说谎）"
        );
    }

    #[test]
    fn open_project_parks_behind_the_unsaved_document_gate() {
        let mut app = sample_app("game_jhyt");
        app.document.current_scene.as_mut().expect("测试场景").dirty = true;
        let error =
            allow_t3(&mut app, json!({"path": "../game_api"})).expect_err("脏文档时不能谎报已切换");
        assert!(error.message().contains("挂起"), "{}", error.message());
        assert!(
            matches!(
                app.pending_document_change,
                Some(crate::actions::PendingChange::SwitchProject { .. })
            ),
            "切工程应停在确认网关里等人裁决"
        );
        assert!(!app.progress.running, "未确认前不得启动后台导入");
        app.cancel_pending_change();
        assert!(app.pending_document_change.is_none());
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

    /// 内置助手生成 UI 预设前必须能拿到契约：`read_resource` 与 MCP `resources/read`
    /// 返回同一份 `rustlua://ui/guide`，否则模型只能猜字段。
    #[test]
    fn read_resource_tool_returns_the_ui_guide_for_preset_generation() {
        let (mut app, _root) = test_app("read-resource");
        let guide = direct(
            &mut app,
            "read_resource",
            json!({"uri": "rustlua://ui/guide"}),
        )
        .unwrap();
        let text = guide.as_str().expect("read_resource 返回文本");
        assert_eq!(text, rustlua_editor_ai::UI_GUIDE);
        assert!(text.contains("UiSpec"));
        assert!(text.len() > 1_000);

        let error =
            direct(&mut app, "read_resource", json!({"uri": "rustlua://nope"})).unwrap_err();
        assert!(error.message().contains("未知资源"), "{}", error.message());
    }

    fn ui_spec_json() -> Value {
        json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "shop"},
            "tokens": {"color.surface": "#101014"},
            "styles": {"card": {"base": {"bg": "@color.surface"}}},
            "root": {
                "key": "root",
                "type": "panel",
                "style": "card",
                "children": [{"key": "title", "type": "text", "text": "Shop"}]
            }
        })
    }

    /// MCP 通道的测试替身：只做 JSON-RPC 编解码 + 同一 `execute_tool`，与
    /// `AiBridge::pump` 的分发形状一致（协议线程 → 主线程 → 宿主）。
    fn mcp_tool_call(
        app: &mut EditorApp,
        policy: GatePolicy,
        name: &str,
        args: Value,
    ) -> Result<Value, Value> {
        let mut host = EditorAiHost { app };
        let response = rustlua_editor_ai::mcp::handle_message(
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {"name": name, "arguments": args}
            }),
            &mut |call| match call {
                rustlua_editor_ai::HostCall::Tool { name, args } => {
                    rustlua_editor_ai::execute_tool(&mut host, policy, &name, &args)
                }
                rustlua_editor_ai::HostCall::Resource { uri } => {
                    Err(ToolError::UnknownTool(format!("resource:{uri}")))
                }
            },
        )
        .expect("tools/call 必须回包");
        let result = response.get("result").expect("tools/call result");
        let mut structured = result["structuredContent"].clone();
        // MCP 把预览图放进 image content block、从 structuredContent 里摘掉 base64；
        // 双通道对拍时把它还原回内部 API 的形状，比较的才是同一份数据。
        if let Some(data) = result["content"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["type"] == "image"))
            .and_then(|image| image["data"].as_str())
        {
            structured["png_base64"] = json!(data);
        }
        if result["isError"].as_bool().unwrap_or(false) {
            Err(structured.get("error").cloned().unwrap_or(structured))
        } else {
            Ok(structured)
        }
    }

    fn write_ui_document(root: &std::path::Path, spec: &Value) -> u64 {
        let spec: rustlua_ui_spec::UiSpec = serde_json::from_value(spec.clone()).unwrap();
        let path = root.join("ui/shop.ui.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let encoded = editor::ui_spec_host::encode_spec(&spec).unwrap();
        std::fs::write(&path, &encoded).unwrap();
        rustlua_ui_spec::revision_hash(&encoded)
    }

    #[test]
    fn ui_create_preset_writes_a_new_document_and_refuses_to_overwrite() {
        let (mut app, root) = test_app("ui-create-preset");
        let created = direct(
            &mut app,
            "ui_create_preset",
            json!({
                "document": "ui/tavern.ui.json",
                "spec": ui_spec_json(),
                "preview": true,
                "width": 320,
                "height": 180
            }),
        )
        .expect("创建 UI 预设");
        assert_eq!(created["document"], "ui/tavern.ui.json");
        assert!(
            created["revision"]
                .as_u64()
                .is_some_and(|revision| revision > 0),
            "{created}"
        );
        assert!(
            created["preview_png_base64"]
                .as_str()
                .is_some_and(|png| png.starts_with("iVBORw0KGgo")),
            "预览必须是真 PNG：{created}"
        );
        let path = root.join("ui/tavern.ui.json");
        assert!(path.is_file(), "接受后必须落盘");
        let loaded = direct(
            &mut app,
            "ui_export_spec",
            json!({"document": "ui/tavern.ui.json"}),
        )
        .expect("回读");
        assert_eq!(loaded["revision"], created["revision"]);

        let error = direct(
            &mut app,
            "ui_create_preset",
            json!({
                "document": "ui/tavern.ui.json",
                "spec": ui_spec_json(),
                "preview": false
            }),
        )
        .expect_err("已存在的文档不得被 create 覆盖");
        assert!(error.message().contains("已存在"), "{}", error.message());
    }

    #[test]
    fn ui_tools_are_backed_by_the_spec_host() {
        let (mut app, root) = test_app("ui-tools");
        let revision = write_ui_document(&root, &ui_spec_json());

        let exported = direct(
            &mut app,
            "ui_export_spec",
            json!({"document": "ui/shop.ui.json"}),
        )
        .expect("export");
        assert_eq!(exported["revision"], revision);
        assert_eq!(exported["spec"]["document"]["name"], "shop");
        assert_eq!(exported["spec"]["root"], ui_spec_json()["root"]);

        let validation = direct(
            &mut app,
            "ui_validate_spec",
            json!({"spec": exported["spec"]}),
        )
        .expect("validate");
        assert_eq!(validation["diagnostics"], json!([]), "{validation}");

        let preview = direct(
            &mut app,
            "ui_preview_spec",
            json!({"spec": exported["spec"], "width": 320, "height": 180}),
        )
        .expect("preview");
        assert!(
            preview["png_base64"]
                .as_str()
                .is_some_and(|png| png.starts_with("iVBORw0KGgo")),
            "{preview}"
        );

        let proposal = direct(
            &mut app,
            "ui_apply_patch",
            json!({
                "document": "ui/shop.ui.json",
                "patch": {
                    "base_revision": revision,
                    "ops": [{
                        "op": "set_event",
                        "key": "title",
                        "event": "click",
                        "function": "shop.open"
                    }]
                }
            }),
        )
        .expect("apply patch");
        assert_eq!(proposal["proposal"]["kind"], "patch");
        let unchanged = direct(
            &mut app,
            "ui_export_spec",
            json!({"document": "ui/shop.ui.json"}),
        )
        .unwrap();
        assert_eq!(unchanged["revision"], revision, "提案未接受前不得写文档");

        let stale = direct(
            &mut app,
            "ui_apply_patch",
            json!({
                "document": "ui/shop.ui.json",
                "patch": {"base_revision": revision + 1, "ops": []}
            }),
        )
        .expect_err("过期 revision 必须拒绝");
        assert!(stale.message().contains("过期"), "{stale:?}");

        let assets = direct(&mut app, "ui_list_assets", json!({})).expect("list assets");
        assert_eq!(assets["assets"], json!([]));

        let policy = GatePolicy {
            allow_t2: true,
            ..GatePolicy::HEADLESS
        };
        let generate = call_tool_direct(
            &mut app,
            policy,
            "ui_generate_assets",
            json!({"plan": [{
                "key": "icon",
                "role": "icon",
                "generate": {"prompt": "green inventory icon", "size": [16, 16], "seed": 3}
            }]}),
        )
        .expect("本地 provider 离线出图");
        let generated = generate["generated"].as_array().expect("generated");
        assert_eq!(generated.len(), 1);
        assert_eq!(generated[0]["key"], "icon");
        assert_eq!(generated[0]["provenance"]["reviewed"], false);
        assert_eq!(generated[0]["provenance"]["source"], "generated");
        assert!(
            root.join("assets/generated/ui/icon.png").exists(),
            "出图产物必须落盘"
        );
        let assets = direct(&mut app, "ui_list_assets", json!({})).expect("list assets");
        assert!(
            assets["assets"].as_array().unwrap().is_empty(),
            "资产未登记进文档时列表为空"
        );

        // 切图 tool：源图 → 裁片 + slices.json，失败时不留输出目录。
        let bad = call_tool_direct(
            &mut app,
            policy,
            "ui_slice_asset",
            json!({
                "source": "assets/generated/ui/icon.png",
                "output_dir": "ui/slices_bad",
                "regions": [{"name": "oops", "rect": [8, 8, 16, 16]}]
            }),
        )
        .expect_err("越界区域必须拒绝");
        assert!(bad.message().contains("超出"), "{bad:?}");
        assert!(
            !root.join("ui/slices_bad").exists(),
            "校验失败不得创建输出目录"
        );

        let sliced = call_tool_direct(
            &mut app,
            policy,
            "ui_slice_asset",
            json!({
                "source": "assets/generated/ui/icon.png",
                "output_dir": "ui/slices",
                "regions": [{"name": "icon_half", "rect": [0, 0, 8, 8]}]
            }),
        )
        .expect("切片应落盘");
        assert_eq!(sliced["source_width"], 16);
        assert_eq!(sliced["source_height"], 16);
        let slices = sliced["slices"].as_array().expect("slices");
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0]["name"], "icon_half");
        assert_eq!(slices[0]["rect"], json!([0, 0, 8, 8]));
        assert!(slices[0]["bytes"].as_u64().unwrap() > 0);
        assert!(root.join("ui/slices/icon_half.png").exists());
        assert!(root.join("ui/slices/slices.json").exists());
    }

    #[test]
    fn ui_promote_asset_requires_review_then_writes_provenance() {
        let (mut app, root) = test_app("ui-assets");
        let mut spec = ui_spec_json();
        spec["assets"] = json!([{
            "key": "shop.bg",
            "role": "background",
            "path": "assets/ui/shop_bg.png"
        }]);
        write_ui_document(&root, &spec);
        let policy = GatePolicy {
            allow_t2: true,
            ..GatePolicy::HEADLESS
        };

        let error = call_tool_direct(
            &mut app,
            policy,
            "ui_promote_asset",
            json!({"asset_id": "shop.bg", "reviewed": false}),
        )
        .expect_err("未审阅必须拒绝");
        assert!(error.message().contains("审阅"), "{error:?}");

        let promoted = call_tool_direct(
            &mut app,
            policy,
            "ui_promote_asset",
            json!({
                "asset_id": "shop.bg",
                "reviewed": true,
                "license": "CC0",
                "used_by": ["shop.root"]
            }),
        )
        .expect("promote");
        assert_eq!(promoted["provenance"]["asset_id"], "shop.bg");
        assert_eq!(promoted["provenance"]["reviewed"], true);

        let listing = direct(
            &mut app,
            "ui_list_assets",
            json!({"document": "ui/shop.ui.json"}),
        )
        .expect("list assets");
        assert_eq!(listing["assets"][0]["provenance"]["license"], "CC0");
    }

    /// M76-00「双通道原则」验收：同一 UI 请求走内部 API 与 MCP（JSON-RPC）时，
    /// 工具名、tier、诊断与提案 diff 必须一致。两条通道共用 `execute_tool` 与宿主，
    /// 本用例证明协议翻译层没有丢参数、吞错误或改写返回。
    #[test]
    fn ui_tool_results_match_between_internal_api_and_mcp() {
        let spec = ui_spec_json();
        let (mut internal_app, internal_root) = test_app("ui-parity-internal");
        let (mut mcp_app, mcp_root) = test_app("ui-parity-mcp");
        let revision = write_ui_document(&internal_root, &spec);
        assert_eq!(write_ui_document(&mcp_root, &spec), revision);
        let allow_writes = GatePolicy {
            allow_t2: true,
            ..GatePolicy::HEADLESS
        };

        // 工具名单：MCP `tools/list` 必须覆盖内部注册表的每一条。
        let listed = rustlua_editor_ai::mcp::handle_message(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            &mut |_call| -> Result<Value, ToolError> { unreachable!("tools/list 不调用宿主") },
        )
        .expect("tools/list 必须回包");
        let listed: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        for name in rustlua_editor_ai::ui_tools::TOOL_NAMES {
            assert!(listed.contains(name), "MCP tools/list 缺少 UI tool：{name}");
        }

        let calls = [
            ("ui_export_spec", json!({"document": "ui/shop.ui.json"})),
            (
                "ui_validate_spec",
                json!({"spec": {
                    "spec_version": 1,
                    "document": {"kind": "widget", "name": "shop"},
                    "root": {"key": "", "type": "panel"}
                }}),
            ),
            (
                "ui_preview_spec",
                json!({"spec": spec.clone(), "width": 64, "height": 32}),
            ),
            (
                "ui_apply_patch",
                json!({
                    "document": "ui/shop.ui.json",
                    "patch": {
                        "base_revision": revision,
                        "ops": [{
                            "op": "set_event",
                            "key": "title",
                            "event": "click",
                            "function": "shop.open"
                        }]
                    }
                }),
            ),
            (
                "ui_apply_spec",
                json!({
                    "document": "ui/shop.ui.json",
                    "base_revision": revision,
                    "spec": spec.clone()
                }),
            ),
            ("ui_list_assets", json!({"document": "ui/shop.ui.json"})),
        ];
        for (name, args) in calls {
            let internal = call_tool_direct(&mut internal_app, allow_writes, name, args.clone())
                .unwrap_or_else(|error| panic!("内部 API {name} 失败：{error:?}"));
            let over_mcp = mcp_tool_call(&mut mcp_app, allow_writes, name, args)
                .unwrap_or_else(|error| panic!("MCP {name} 失败：{error}"));
            assert_eq!(over_mcp, internal, "{name} 的双通道返回不一致");
        }
        // 提案不只是"形状相同"：最终 diff 也必须逐字段一致。
        let internal_diff = call_tool_direct(
            &mut internal_app,
            allow_writes,
            "ui_apply_patch",
            json!({
                "document": "ui/shop.ui.json",
                "patch": {
                    "base_revision": revision,
                    "ops": [{
                        "op": "set_event",
                        "key": "root",
                        "event": "click",
                        "function": "shop.root"
                    }]
                }
            }),
        )
        .expect("内部根节点提案");
        let mcp_diff = mcp_tool_call(
            &mut mcp_app,
            allow_writes,
            "ui_apply_patch",
            json!({
                "document": "ui/shop.ui.json",
                "patch": {
                    "base_revision": revision,
                    "ops": [{
                        "op": "set_event",
                        "key": "root",
                        "event": "click",
                        "function": "shop.root"
                    }]
                }
            }),
        )
        .expect("MCP 根节点提案");
        assert_eq!(
            mcp_diff["proposal"]["diff"], internal_diff["proposal"]["diff"],
            "同一请求的提案 diff 必须一致"
        );

        // tier 门在两通道上必须同样生效（T2 未被策略放行 → 同一个 NeedsConfirmation）。
        for (name, args) in [
            ("ui_generate_assets", json!({"plan": []})),
            (
                "ui_promote_asset",
                json!({"asset_id": "shop.bg", "reviewed": true}),
            ),
            (
                "ui_slice_asset",
                json!({
                    "source": "assets/generated/ui/icon.png",
                    "output_dir": "ui/slices",
                    "regions": [{"name": "a", "rect": [0, 0, 1, 1]}]
                }),
            ),
        ] {
            let internal =
                call_tool_direct(&mut internal_app, GatePolicy::HEADLESS, name, args.clone())
                    .expect_err("无策略放行时必须被确认门拒绝");
            let over_mcp = mcp_tool_call(&mut mcp_app, GatePolicy::HEADLESS, name, args)
                .expect_err("MCP 也必须被确认门拒绝");
            assert_eq!(
                over_mcp,
                serde_json::to_value(&internal).expect("ToolError 可序列化"),
                "{name} 的 tier 门在两通道上不一致"
            );
            assert_eq!(
                internal,
                ToolError::NeedsConfirmation {
                    tool: name.to_owned(),
                    tier: "T2",
                }
            );
        }
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
    fn ui_tool_replay_fixture_runs_headless() {
        let fixture: ReplayFixture =
            serde_json::from_str(include_str!("../tests/fixtures/ai_replay_ui_spec.json"))
                .expect("ui fixture");
        let (mut app, _root) = test_app("ui-replay-static");
        let mut host = EditorAiHost { app: &mut app };
        let report = run_replay(&fixture, &mut host).expect("replay");
        assert!(report.ok(), "{report:?}");
        assert_eq!(report.steps, 3);
    }

    #[test]
    fn ui_patch_replay_is_deterministic_across_fresh_sessions() {
        let spec = ui_spec_json();
        let (mut first, root_a) = test_app("ui-replay-a");
        let (mut second, root_b) = test_app("ui-replay-b");
        let revision = write_ui_document(&root_a, &spec);
        assert_eq!(write_ui_document(&root_b, &spec), revision);
        let fixture: ReplayFixture = serde_json::from_value(json!({
            "name": "ui_patch",
            "steps": [
                {"tool": "ui_export_spec", "args": {"document": "ui/shop.ui.json"}},
                {
                    "tool": "ui_apply_patch",
                    "args": {
                        "document": "ui/shop.ui.json",
                        "patch": {
                            "base_revision": revision,
                            "ops": [{
                                "op": "set_event",
                                "key": "title",
                                "event": "click",
                                "function": "shop.open"
                            }]
                        }
                    },
                    "expect_json": {
                        "proposal": {
                            "kind": "patch",
                            "diff": {"nodes_changed": 1}
                        }
                    }
                },
                {"tool": "ui_export_spec", "args": {"document": "ui/shop.ui.json"}}
            ]
        }))
        .unwrap();
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
            first.ai_snapshot_for_test(),
            second.ai_snapshot_for_test(),
            "同一 UI fixture 两次回放必须得到同一场景快照"
        );
        assert_eq!(
            first.document.current_scene.as_ref().unwrap().scene,
            second.document.current_scene.as_ref().unwrap().scene
        );
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
