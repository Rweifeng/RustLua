//! P3-E03-2：`.vgraph` 编辑器文档（加载 / 编辑 / 撤销 / 原子保存 + 生成 Lua）。
//!
//! 边界：
//!
//! - **只碰工程内的 `.vgraph` 与 `scripts/generated/<name>.lua`**：路径必须是相对路径，
//!   不允许 `..`、绝对路径或盘符；解析后的落点永远在项目根下。
//! - **生成失败不写盘**：先 `compile`（内部先 `validate`），失败只更新 [`GraphDocument::errors`]，
//!   旧 `.vgraph` 与旧 Lua 都保持原样；成功时两份文件都先写临时文件再原子替换。
//! - **编辑不脏场景文档**：这里只持有图的纯数据与撤销栈，不引用 `SceneDocument`、不发
//!   `EditorCommand`，因此编辑器面板的架构棘轮（`panels_only_emit_actions`）不受影响。
//! - 撤销粒度：结构操作每次一步；参数输入用 [`GraphDocument::set_param_coalesced`] 把同一字段
//!   的连续输入合并成一步（切换字段或做结构操作即断开）。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rustlua_visual_script::{
    Graph, GraphError, Node, ParamSpec, PortRef, PortType, ValueType, compile, node_spec,
};

/// 撤销栈上限：图最多 256 节点，一步一份快照足够便宜；设上限只防长会话无界增长。
const MAX_UNDO: usize = 64;

/// 面板持有的图文档。`dirty` 由 `graph != saved` 派生，撤销回到保存点会自动变干净。
pub(crate) struct GraphDocument {
    pub relative_path: PathBuf,
    pub graph: Graph,
    saved: Graph,
    undo: Vec<Graph>,
    redo: Vec<Graph>,
    pub errors: Vec<GraphError>,
    pub compiled_lua: Option<String>,
    pub generated_path: Option<PathBuf>,
    /// 上一次参数合并键；连续编辑同一 `(node, param)` 不再压撤销栈。
    last_param: Option<(String, String)>,
}

pub(crate) struct SaveReport {
    pub vgraph_bytes: usize,
    pub lua_bytes: usize,
    pub generated: PathBuf,
}

#[derive(Debug)]
pub(crate) enum SaveFailure {
    /// 校验/代码生成失败：磁盘上的旧文件没被碰过。
    Invalid(Vec<GraphError>),
    Io(String),
}

impl GraphDocument {
    pub(crate) fn load(root: &Path, relative: &Path) -> Result<Self, String> {
        let path = resolve_vgraph(root, relative)?;
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
        let graph = Graph::from_json(&text).map_err(|error| error.to_string())?;
        Ok(Self {
            relative_path: relative.to_path_buf(),
            saved: graph.clone(),
            graph,
            undo: Vec::new(),
            redo: Vec::new(),
            errors: Vec::new(),
            compiled_lua: None,
            generated_path: None,
            last_param: None,
        })
    }

    /// 生成器头部里的来源路径（统一 `/`，Windows 上也能得到稳定 golden）。
    pub(crate) fn source(&self) -> String {
        self.relative_path.to_string_lossy().replace('\\', "/")
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.graph != self.saved
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub(crate) fn generated_relative(&self) -> PathBuf {
        let name = sanitize_name(&self.graph.name);
        Path::new("scripts")
            .join("generated")
            .join(format!("{name}.lua"))
    }

    /// 只校验 + 生成到内存，不写盘（工具栏「编译」）。
    pub(crate) fn check(&mut self) -> bool {
        match compile(&self.graph, &self.source()) {
            Ok(lua) => {
                self.compiled_lua = Some(lua);
                self.errors.clear();
                true
            }
            Err(errors) => {
                self.compiled_lua = None;
                self.errors = errors;
                false
            }
        }
    }

    /// 校验 + 生成 + 原子落盘。生成失败时磁盘上的旧文件保持不变。
    pub(crate) fn save(&mut self, root: &Path) -> Result<SaveReport, SaveFailure> {
        let lua = match compile(&self.graph, &self.source()) {
            Ok(lua) => lua,
            Err(errors) => {
                self.errors = errors.clone();
                self.compiled_lua = None;
                return Err(SaveFailure::Invalid(errors));
            }
        };
        let graph_path = resolve_vgraph(root, &self.relative_path).map_err(SaveFailure::Io)?;
        let generated =
            resolve_relative(root, &self.generated_relative()).map_err(SaveFailure::Io)?;
        let graph_text = self.graph.to_json();
        let old_graph = fs::read_to_string(&graph_path).ok();

        let graph_temp = stage(&graph_path, &graph_text).map_err(SaveFailure::Io)?;
        let lua_temp = match stage(&generated, &lua) {
            Ok(temp) => temp,
            Err(error) => {
                let _ = fs::remove_file(&graph_temp);
                return Err(SaveFailure::Io(error));
            }
        };
        if let Err(error) = commit(&graph_temp, &graph_path) {
            let _ = fs::remove_file(&lua_temp);
            return Err(SaveFailure::Io(error));
        }
        if let Err(error) = commit(&lua_temp, &generated) {
            // 第二份替换失败：尽量把第一份还原，避免"图是新的、生成物是旧的"。
            if let Some(old) = old_graph {
                let _ = write_atomic(&graph_path, &old);
            }
            return Err(SaveFailure::Io(error));
        }

        self.saved = self.graph.clone();
        self.errors.clear();
        self.compiled_lua = Some(lua.clone());
        self.generated_path = Some(generated.clone());
        Ok(SaveReport {
            vgraph_bytes: graph_text.len(),
            lua_bytes: lua.len(),
            generated,
        })
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.graph.clone());
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.last_param = None;
    }

    /// 画布拖动开始时调用一次：整段拖动只压一步撤销。
    pub(crate) fn begin_gesture(&mut self) {
        self.checkpoint();
    }

    pub(crate) fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.graph, previous);
        self.redo.push(current);
        self.last_param = None;
        true
    }

    pub(crate) fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.graph, next);
        self.undo.push(current);
        self.last_param = None;
        true
    }

    /// 添加一个节点；事件节点每个事件只允许一个。返回稳定 id。
    pub(crate) fn add_node(&mut self, kind: &str, position: [f32; 2]) -> Result<String, String> {
        let spec = node_spec(kind).ok_or_else(|| format!("未注册的节点类型 `{kind}`"))?;
        if let Some(event) = spec.event
            && self.graph.nodes.iter().any(|node| {
                node_spec(&node.kind).and_then(|candidate| candidate.event) == Some(event)
            })
        {
            return Err(format!(
                "事件 `{}` 已经有一个入口节点",
                event.function_name()
            ));
        }
        let id = unique_node_id(&self.graph, kind);
        self.checkpoint();
        self.graph.nodes.push(Node {
            id: id.clone(),
            kind: kind.to_owned(),
            position,
            params: default_params(spec.params),
        });
        Ok(id)
    }

    pub(crate) fn remove_nodes(&mut self, ids: &BTreeSet<String>) -> usize {
        let before = self.graph.nodes.len();
        if !self.graph.nodes.iter().any(|node| ids.contains(&node.id)) {
            return 0;
        }
        self.checkpoint();
        self.graph.nodes.retain(|node| !ids.contains(&node.id));
        self.graph
            .edges
            .retain(|edge| !ids.contains(&edge.from.node) && !ids.contains(&edge.to.node));
        before - self.graph.nodes.len()
    }

    /// 拖动过程中的位置更新；撤销快照由 [`Self::begin_gesture`] 负责。
    pub(crate) fn move_node(&mut self, id: &str, position: [f32; 2]) {
        if let Some(node) = self.graph.nodes.iter_mut().find(|node| node.id == id) {
            node.position = position;
            self.last_param = None;
        }
    }

    /// 连线。数据输入只允许一条入边；流程输入允许多条（汇合/回边）。
    pub(crate) fn connect(&mut self, from: PortRef, to: PortRef) -> Result<(), String> {
        if from.node == to.node {
            return Err("不能把节点连到它自己".to_owned());
        }
        let from_spec = node_spec(
            &self
                .graph
                .node(&from.node)
                .ok_or_else(|| format!("起点节点 `{}` 不存在", from.node))?
                .kind,
        )
        .and_then(|spec| spec.output(&from.port))
        .ok_or_else(|| format!("起点端口 `{}.{}` 不存在", from.node, from.port))?;
        let to_spec = node_spec(
            &self
                .graph
                .node(&to.node)
                .ok_or_else(|| format!("终点节点 `{}` 不存在", to.node))?
                .kind,
        )
        .and_then(|spec| spec.input(&to.port))
        .ok_or_else(|| format!("终点端口 `{}.{}` 不存在", to.node, to.port))?;
        if !to_spec.ty.accepts(from_spec.ty) {
            return Err(format!(
                "类型不匹配：`{}.{}` 是 {}，`{}.{}` 需要 {}",
                from.node,
                from.port,
                from_spec.ty.label(),
                to.node,
                to.port,
                to_spec.ty.label()
            ));
        }
        if self
            .graph
            .edges
            .iter()
            .any(|edge| edge.from == from && edge.to == to)
        {
            return Err("这条边已经存在".to_owned());
        }
        if !matches!(from_spec.ty, PortType::Flow)
            && self.graph.edges.iter().any(|edge| edge.to == to)
        {
            return Err(format!("数据输入 `{}.{}` 已经连接", to.node, to.port));
        }
        self.checkpoint();
        self.graph
            .edges
            .push(rustlua_visual_script::Edge { from, to });
        Ok(())
    }

    pub(crate) fn disconnect_input(&mut self, node: &str, port: &str) -> usize {
        let before = self.graph.edges.len();
        if !self
            .graph
            .edges
            .iter()
            .any(|edge| edge.to.node == node && edge.to.port == port)
        {
            return 0;
        }
        self.checkpoint();
        self.graph
            .edges
            .retain(|edge| !(edge.to.node == node && edge.to.port == port));
        before - self.graph.edges.len()
    }

    /// 参数编辑：同一 `(node, param)` 的连续输入合并为一步撤销。
    pub(crate) fn set_param_coalesced(
        &mut self,
        node: &str,
        param: &str,
        value: serde_json::Value,
    ) {
        let key = (node.to_owned(), param.to_owned());
        if self.last_param.as_ref() != Some(&key) {
            self.checkpoint();
            self.last_param = Some(key);
        }
        if let Some(target) = self.graph.nodes.iter_mut().find(|target| target.id == node) {
            target.params.insert(param.to_owned(), value);
        }
    }
}

fn default_params(specs: &[ParamSpec]) -> serde_json::Map<String, serde_json::Value> {
    let mut params = serde_json::Map::new();
    for spec in specs {
        let value = match spec.ty {
            ValueType::Bool => serde_json::Value::Bool(false),
            ValueType::Number => serde_json::json!(0.0),
            ValueType::String => {
                // `math.op` 的唯一合法值是四则运算；给个能直接编译的默认值。
                if spec.name == "op" {
                    serde_json::json!("add")
                } else {
                    serde_json::json!("")
                }
            }
            _ => serde_json::Value::Null,
        };
        params.insert(spec.name.to_owned(), value);
    }
    params
}

fn unique_node_id(graph: &Graph, kind: &str) -> String {
    let base = kind
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if graph.node(&base).is_none() {
        return base;
    }
    for suffix in 2..=u32::MAX {
        let candidate = format!("{base}{suffix}");
        if graph.node(&candidate).is_none() {
            return candidate;
        }
    }
    unreachable!("节点 id 后缀空间耗尽")
}

fn sanitize_name(name: &str) -> String {
    let mapped = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let trimmed = mapped.trim_matches('_');
    if trimmed.is_empty() {
        "graph".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn resolve_relative(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "只接受工程内相对路径（不允许绝对路径、盘符或 `..`）：{}",
            relative.display()
        ));
    }
    Ok(root.join(relative))
}

fn resolve_vgraph(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative.extension().and_then(|value| value.to_str()) != Some("vgraph") {
        return Err(format!("不是 `.vgraph` 文件：{}", relative.display()));
    }
    resolve_relative(root, relative)
}

fn stage(path: &Path, text: &str) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("路径没有父目录：{}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("创建 {} 失败：{error}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("vgraph");
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let temp = parent.join(format!(".{file_name}.{token}.tmp"));
    fs::write(&temp, text).map_err(|error| format!("写入 {} 失败：{error}", temp.display()))?;
    Ok(temp)
}

fn commit(temp: &Path, target: &Path) -> Result<(), String> {
    fs::rename(temp, target).map_err(|error| {
        let _ = fs::remove_file(temp);
        format!("替换 {} 失败：{error}", target.display())
    })
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let temp = stage(path, text)?;
    commit(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_visual_script::{Edge, ValueType, Variable};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let token = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default();
            let path = std::env::temp_dir().join(format!(
                "rustlua-graph-editor-{}-{token}",
                std::process::id()
            ));
            fs::create_dir_all(path.join("assets/graphs")).expect("创建临时工程");
            Self(path)
        }

        fn root(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample_graph() -> Graph {
        Graph {
            version: 1,
            name: "demo".to_owned(),
            variables: vec![Variable {
                name: "score".to_owned(),
                value_type: ValueType::Number,
                default: Some(serde_json::json!(0)),
            }],
            nodes: vec![
                Node {
                    id: "start".to_owned(),
                    kind: "on_start".to_owned(),
                    position: [10.0, 20.0],
                    params: serde_json::Map::new(),
                },
                Node {
                    id: "save".to_owned(),
                    kind: "save_game".to_owned(),
                    position: [240.0, 20.0],
                    params: serde_json::Map::new(),
                },
            ],
            edges: vec![Edge {
                from: PortRef {
                    node: "start".to_owned(),
                    port: "then".to_owned(),
                },
                to: PortRef {
                    node: "save".to_owned(),
                    port: "in".to_owned(),
                },
            }],
        }
    }

    fn write_graph(root: &Path, relative: &str, graph: &Graph) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("父目录")).expect("建目录");
        fs::write(path, graph.to_json()).expect("写 fixture");
    }

    #[test]
    fn graph_editor_roundtrip_save_reload_and_generate() {
        let temp = TempDir::new();
        write_graph(temp.root(), "assets/graphs/demo.vgraph", &sample_graph());
        let relative = Path::new("assets/graphs/demo.vgraph");
        let mut document = GraphDocument::load(temp.root(), relative).expect("加载");
        assert!(!document.is_dirty());

        document
            .add_node("number", [240.0, 160.0])
            .expect("添加数字节点");
        assert!(document.is_dirty(), "编辑后必须变脏");
        let report = document.save(temp.root()).expect("保存");
        assert!(!document.is_dirty(), "保存后必须回到干净状态");
        assert!(report.vgraph_bytes > 0 && report.lua_bytes > 0);
        let generated = temp.root().join("scripts/generated/demo.lua");
        assert!(generated.is_file(), "必须生成 scripts/generated/demo.lua");
        let lua = fs::read_to_string(&generated).expect("读生成物");
        assert!(lua.contains("function M.on_start(owner)"));
        assert!(lua.contains("-- generated from assets/graphs/demo.vgraph"));

        let reloaded = GraphDocument::load(temp.root(), relative).expect("重新加载");
        assert_eq!(reloaded.graph.nodes.len(), 3);
        let number = reloaded.graph.node("number").expect("数字节点还在");
        assert_eq!(number.position, [240.0, 160.0]);
        assert_eq!(reloaded.graph.variables.len(), 1);
    }

    #[test]
    fn graph_editor_undo_redo_returns_to_clean_save_point() {
        let temp = TempDir::new();
        write_graph(temp.root(), "assets/graphs/demo.vgraph", &sample_graph());
        let mut document =
            GraphDocument::load(temp.root(), Path::new("assets/graphs/demo.vgraph")).expect("加载");
        let id = document
            .add_node("bool", [30.0, 200.0])
            .expect("添加布尔节点");
        assert!(document.is_dirty());
        assert!(document.undo());
        assert!(!document.is_dirty(), "撤销回到保存点必须干净");
        assert!(document.graph.node(&id).is_none());
        assert!(document.redo());
        assert!(document.is_dirty());
        assert!(document.graph.node(&id).is_some());

        document.begin_gesture();
        document.move_node(&id, [200.0, 300.0]);
        assert_eq!(
            document.graph.node(&id).expect("节点").position,
            [200.0, 300.0]
        );
        assert!(document.undo());
        assert_eq!(
            document.graph.node(&id).expect("节点").position,
            [30.0, 200.0]
        );
    }

    #[test]
    fn graph_editor_invalid_graph_keeps_old_files() {
        let temp = TempDir::new();
        write_graph(temp.root(), "assets/graphs/demo.vgraph", &sample_graph());
        let mut document =
            GraphDocument::load(temp.root(), Path::new("assets/graphs/demo.vgraph")).expect("加载");
        document.save(temp.root()).expect("先写一份有效文件");
        let graph_path = temp.root().join("assets/graphs/demo.vgraph");
        let lua_path = temp.root().join("scripts/generated/demo.lua");
        let old_graph = fs::read_to_string(&graph_path).expect("旧图");
        let old_lua = fs::read_to_string(&lua_path).expect("旧 Lua");

        document.graph.edges.push(Edge {
            from: PortRef {
                node: "start".to_owned(),
                port: "then".to_owned(),
            },
            to: PortRef {
                node: "missing".to_owned(),
                port: "in".to_owned(),
            },
        });
        let failure = document.save(temp.root());
        assert!(matches!(failure, Err(SaveFailure::Invalid(_))));
        assert!(!document.errors.is_empty());
        assert_eq!(fs::read_to_string(&graph_path).expect("旧图"), old_graph);
        assert_eq!(fs::read_to_string(&lua_path).expect("旧 Lua"), old_lua);
    }

    #[test]
    fn graph_editor_rejects_paths_outside_project() {
        let temp = TempDir::new();
        let error = GraphDocument::load(temp.root(), Path::new("../escape.vgraph"))
            .err()
            .expect("必须拒绝 `..`");
        assert!(error.contains("相对路径"), "{error}");
        let absolute = temp.root().join("assets/graphs/demo.vgraph");
        assert!(
            GraphDocument::load(temp.root(), &absolute).is_err(),
            "必须拒绝绝对路径"
        );
        assert!(
            GraphDocument::load(temp.root(), Path::new("assets/graphs/demo.txt")).is_err(),
            "必须拒绝非 .vgraph"
        );
    }

    #[test]
    fn graph_editor_connect_typechecks_and_coalesces_params() {
        let temp = TempDir::new();
        write_graph(temp.root(), "assets/graphs/demo.vgraph", &sample_graph());
        let mut document =
            GraphDocument::load(temp.root(), Path::new("assets/graphs/demo.vgraph")).expect("加载");
        let number = document
            .add_node("number", [20.0, 160.0])
            .expect("数字节点");
        let save = document.graph.node("save").expect("save 节点").id.clone();
        let mismatch = document.connect(
            PortRef {
                node: number.clone(),
                port: "value".to_owned(),
            },
            PortRef {
                node: save.clone(),
                port: "in".to_owned(),
            },
        );
        assert!(mismatch.is_err(), "数值不能接流程输入");

        for value in [1.0, 2.0, 3.0] {
            document.set_param_coalesced(&number, "value", serde_json::json!(value));
        }
        assert_eq!(
            document.graph.node(&number).expect("节点").params["value"],
            serde_json::json!(3.0)
        );
        assert!(document.undo());
        assert_eq!(
            document.graph.node(&number).expect("节点").params["value"],
            serde_json::json!(0.0),
            "同字段连续输入只占一步撤销"
        );
        assert!(document.graph.node(&number).is_some());
        assert!(document.undo());
        assert!(
            document.graph.node(&number).is_none(),
            "再撤销一步才撤掉添加"
        );
    }
}
