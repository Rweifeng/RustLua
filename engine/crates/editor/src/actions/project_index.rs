//! 全工程搜索索引：一次读盘，把"能搜的东西"摊平成可过滤的行。
//!
//! 四个来源（与面板头一致）：
//! 1. **文档组件字段**：每个 `.scene`/`.prefab` 的每个实体、每个组件字段（schema 行 +
//!    手写面板组件的 `key=value`）。路径就是 Inspector 的 `Component.field`。
//! 2. **实体/文档元数据**：实体名、实体 `metadata`、文档 `metadata`（含 `prefab.*` 谱系键）。
//! 3. **资源路径**：资源索引里的每一条（与断链检查窗同一份事实源）。
//! 4. **Lua 文本**：工程内 `.lua` 的每一行（行号与只读查看器的切分口径同源）。
//!
//! 三条口径：
//! - **当前打开的文档用内存版**（含未保存编辑），磁盘上那一份不重复计——与断链审计、
//!   依赖反查同一条规则；否则"刚改的字段搜不到、已改掉的旧值还搜得到"。
//! - **快照不是实时视图**：索引在显式建立/刷新时生成，之后搜索只读内存；文档在别处改了
//!   要刷新索引才看得到。这与断链检查窗的"一次扫描结果"同一语义。
//! - **命中封顶但计数不封顶**：面板只渲染前 [`MAX_SEARCH_HITS`] 行，标题仍报真实总数——
//!   不制造"只有这么几条"的假象。

use editor::SceneDocument;
use rustlua_scene::prefab::Prefab;
use rustlua_scene::scene::{SceneEntity, SceneEntityId};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 面板最多渲染的命中行数（计数照实报）。
pub(crate) const MAX_SEARCH_HITS: usize = 400;

/// 命中类别（面板用它做前缀、测试用它锁定来源）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchKind {
    Field,
    Metadata,
    Entity,
    Document,
    Resource,
    Script,
}

impl SearchKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Field => "字段",
            Self::Metadata => "元数据",
            Self::Entity => "实体",
            Self::Document => "文档",
            Self::Resource => "资源",
            Self::Script => "脚本",
        }
    }
}

/// 双击一行之后要落到哪里。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SearchTarget {
    /// 文档里的实体字段。`schema_field` 为假表示该字段由手写面板显示（只能定位到实体）。
    Field {
        document: String,
        entity: SceneEntityId,
        field_path: String,
        schema_field: bool,
    },
    /// 文档里的实体（实体名 / 实体元数据）。
    Entity {
        document: String,
        entity: SceneEntityId,
    },
    /// 文档本身（文档路径 / 文档级元数据）。
    Document { document: String },
    /// 资源面板里的一个资源。
    Resource { path: String },
    /// Lua 源码的一行（只读查看器）。
    Script { path: String, line: u32 },
}

impl SearchTarget {
    /// 这一行落在**哪份编辑文档**上（资源/脚本命中为 `None`）。
    ///
    /// 跳转要先保证这份文档是当前的（可能要换文档），面板与 shell 共用这一个判据。
    pub(crate) fn document(&self) -> Option<&str> {
        match self {
            Self::Field { document, .. }
            | Self::Entity { document, .. }
            | Self::Document { document } => Some(document),
            Self::Resource { .. } | Self::Script { .. } => None,
        }
    }
}

/// 一条搜索结果。展示需要的零件都留着，面板不回头再查索引。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchHit {
    pub kind: SearchKind,
    /// 文档（工程内相对路径，正斜杠）；资源/脚本命中为 `None`。
    pub document: Option<String>,
    pub entity: Option<SceneEntityId>,
    pub entity_name: String,
    /// 字段路径或元数据键；其余类别为空。
    pub field: String,
    /// 取值 / 行文本片段。
    pub detail: String,
    /// 资源或脚本路径。
    pub path: Option<String>,
    /// Lua 行号（1 起）。
    pub line: Option<u32>,
    pub target: SearchTarget,
}

impl SearchHit {
    /// 一行文字：面板与测试共用同一种“文档 ▸ 实体 ▸ 字段”口径。
    pub(crate) fn location_text(&self) -> String {
        let mut out = String::new();
        if let Some(document) = &self.document {
            out.push_str(document);
            if let Some(entity) = self.entity {
                out.push_str(&format!(" ▸ E#{}", entity.0));
                if !self.entity_name.is_empty() {
                    out.push(' ');
                    out.push_str(&self.entity_name);
                }
            }
            if !self.field.is_empty() {
                out.push_str(" ▸ ");
                out.push_str(&self.field);
            }
        } else if let Some(path) = &self.path {
            out.push_str(path);
            if let Some(line) = self.line {
                out.push(':');
                out.push_str(&line.to_string());
            }
        }
        out
    }
}

/// 搜索结果（总数为封顶前的真实条数）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    pub total: usize,
}

/// 一份文档的实体与元数据（场景与 Prefab 形状相同，只是解析器不同）。
pub(crate) struct DocumentSnapshot {
    pub entities: BTreeMap<SceneEntityId, SceneEntity>,
    pub metadata: BTreeMap<String, String>,
}

/// 按扩展名选解析器：`.prefab` 必须走 `Prefab::load`——场景解析器读不了 `prefab_version=`。
pub(crate) fn load_document(path: &Path) -> Result<DocumentSnapshot, String> {
    let is_prefab = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("prefab"));
    if is_prefab {
        Prefab::load(path)
            .map(|prefab| DocumentSnapshot {
                entities: prefab.entities,
                metadata: prefab.metadata,
            })
            .map_err(|error| error.to_string())
    } else {
        rustlua_scene::scene::Scene::load(path)
            .map(|scene| DocumentSnapshot {
                entities: scene.entities,
                metadata: scene.metadata,
            })
            .map_err(|error| error.to_string())
    }
}

/// 工程内相对路径（正斜杠）。
pub(crate) fn relative_text(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexedField {
    pub document: String,
    pub entity: SceneEntityId,
    pub entity_name: String,
    pub component: String,
    pub field: String,
    /// `Component.field`（Inspector 过滤与跳转共用这个字符串）。
    pub path: String,
    pub value: String,
    /// 是否带 `ProjectPath`/`OptionalProjectPath` 规则（断链审计只认这些）。
    pub path_rule: bool,
    /// 是否由通用字段编辑器显示（假 = 手写面板，跳转只能定位到实体）。
    pub schema_field: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexedMetadata {
    pub document: String,
    pub entity: Option<SceneEntityId>,
    pub entity_name: String,
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexedEntity {
    pub document: String,
    pub id: SceneEntityId,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexedLuaFile {
    pub path: String,
    pub lines: Vec<String>,
}

/// 一次建立的全工程索引。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProjectIndex {
    /// 索引属于哪个工程（换工程后旧索引自动失效）。
    pub root: PathBuf,
    pub fields: Vec<IndexedField>,
    pub metadata: Vec<IndexedMetadata>,
    pub entities: Vec<IndexedEntity>,
    pub documents: Vec<String>,
    pub resources: Vec<String>,
    pub lua: Vec<IndexedLuaFile>,
    pub scanned_documents: usize,
    pub scanned_lua_files: usize,
    /// 读不动的**文档**（解析失败）。断链审计报的就是这个数。
    pub unreadable: Vec<String>,
    /// 读不动的**脚本**（多数不是 UTF-8）。与文档分开：否则搜索窗说"N 份读不动"，
    /// 断链窗照着同一句话去解释就会把脚本算进文档数。
    pub unreadable_scripts: Vec<String>,
}

impl ProjectIndex {
    /// 读全工程。`open` 是当前编辑文档（用内存版；磁盘上那一份跳过）。
    pub(crate) fn scan(
        root: &Path,
        open: Option<&SceneDocument>,
        mut resources: Vec<String>,
    ) -> Self {
        // 资源列表按路径排序去重：命中顺序必须可复现（测试要逐条断言命中集合）。
        resources.sort();
        resources.dedup();
        let mut index = Self {
            root: root.to_path_buf(),
            resources,
            ..Self::default()
        };
        let open_path = open.and_then(|document| document.path.clone());
        for path in editor::project_scene_documents(root) {
            if open_path
                .as_ref()
                .is_some_and(|open| editor::paths_match(open, &path))
            {
                continue;
            }
            match load_document(&path) {
                Ok(document) => {
                    index.scanned_documents += 1;
                    let label = relative_text(root, &path);
                    index.absorb(&document.entities, &document.metadata, &label);
                }
                Err(error) => index
                    .unreadable
                    .push(format!("{}（{error}）", relative_text(root, &path))),
            }
        }
        if let Some(document) = open {
            index.scanned_documents += 1;
            let label = document
                .path
                .as_ref()
                .map(|path| relative_text(root, path))
                .unwrap_or_else(|| "未命名场景".to_owned());
            index.absorb(&document.scene.entities, &document.scene.metadata, &label);
        }
        let mut lua_files = Vec::new();
        collect_lua_files(root, &mut lua_files, 0);
        lua_files.sort();
        for path in lua_files {
            let label = relative_text(root, &path);
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    index.scanned_lua_files += 1;
                    index.lua.push(IndexedLuaFile {
                        path: label,
                        lines: crate::panels::lua_source_view::split_source(&text),
                    });
                }
                Err(error) => index.unreadable_scripts.push(format!("{label}（{error}）")),
            }
        }
        index
    }

    /// 换工程后旧索引不能再用（路径与内容都指向另一个工程）。
    pub(crate) fn belongs_to(&self, root: &Path) -> bool {
        self.root == root
    }

    fn absorb(
        &mut self,
        entities: &BTreeMap<SceneEntityId, SceneEntity>,
        document_metadata: &BTreeMap<String, String>,
        document: &str,
    ) {
        self.documents.push(document.to_owned());
        for entity in entities.values() {
            self.entities.push(IndexedEntity {
                document: document.to_owned(),
                id: entity.id,
                name: entity.name.clone(),
            });
            for (key, value) in &entity.metadata {
                self.metadata.push(IndexedMetadata {
                    document: document.to_owned(),
                    entity: Some(entity.id),
                    entity_name: entity.name.clone(),
                    key: key.clone(),
                    value: value.clone(),
                });
            }
            let rows = editor::rows_for_entity(entity);
            let covered: BTreeSet<&str> = rows.iter().map(|row| row.component).collect();
            for row in rows {
                self.fields.push(IndexedField {
                    document: document.to_owned(),
                    entity: entity.id,
                    entity_name: entity.name.clone(),
                    component: row.component.to_owned(),
                    field: row.field.to_owned(),
                    path: row.path,
                    value: row.text,
                    path_rule: row.path_rule,
                    schema_field: true,
                });
            }
            // 手写面板组件（`Script`/`Tilemap`/`Container`…）没有 schema 行；
            // 按 `key=value` 摊平，至少让"谁在用这个脚本/图集"能被搜到。
            for (component, payload) in &entity.components {
                if covered.contains(component.as_str()) {
                    continue;
                }
                for (field, value) in raw_component_fields(payload) {
                    let path = format!("{component}.{field}");
                    self.fields.push(IndexedField {
                        document: document.to_owned(),
                        entity: entity.id,
                        entity_name: entity.name.clone(),
                        component: component.clone(),
                        field,
                        path,
                        value,
                        path_rule: false,
                        schema_field: false,
                    });
                }
            }
        }
        for (key, value) in document_metadata {
            self.metadata.push(IndexedMetadata {
                document: document.to_owned(),
                entity: None,
                entity_name: String::new(),
                key: key.clone(),
                value: value.clone(),
            });
        }
    }

    /// 多词 AND、大小写不敏感：与 Inspector 字段搜索同一语义（空查询不返回全部）。
    ///
    /// 每一类只搜**自己的那一列**（字段=组件.字段+值，元数据=键+值，实体=名字，
    /// 文档=路径，资源=路径，脚本=行文本）：这样"搜一个实体名"只会出一行实体，
    /// 而不是把那个实体（或那份文档）的十几行全部倒进清单。
    pub(crate) fn search(&self, query: &str) -> SearchOutcome {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if terms.is_empty() {
            return SearchOutcome::default();
        }
        let mut outcome = SearchOutcome::default();
        // **每类只搜自己那一列**：字段/元数据的 haystack 里不放文档名，也不放实体名。
        // 放了就会让"搜一个实体名"变成"把这个实体的每一行都列出来"（一个实体十几行），
        // 文档名同理刷屏；而按实体名/文档名找东西本来就是「实体」「文档」那两类的职责。
        // 行的显示与跳转仍然带着 `document` / `entity_name`（读得懂、跳得准）。
        for field in &self.fields {
            let haystack = format!("{} {} {}", field.component, field.field, field.value);
            if !matches_terms(&haystack, &terms) {
                continue;
            }
            record(
                &mut outcome,
                SearchHit {
                    kind: SearchKind::Field,
                    document: Some(field.document.clone()),
                    entity: Some(field.entity),
                    entity_name: field.entity_name.clone(),
                    field: field.path.clone(),
                    detail: field.value.clone(),
                    path: None,
                    line: None,
                    target: SearchTarget::Field {
                        document: field.document.clone(),
                        entity: field.entity,
                        field_path: field.path.clone(),
                        schema_field: field.schema_field,
                    },
                },
            );
        }
        for metadata in &self.metadata {
            let haystack = format!("{} {}", metadata.key, metadata.value);
            if !matches_terms(&haystack, &terms) {
                continue;
            }
            let target = match metadata.entity {
                Some(entity) => SearchTarget::Entity {
                    document: metadata.document.clone(),
                    entity,
                },
                None => SearchTarget::Document {
                    document: metadata.document.clone(),
                },
            };
            record(
                &mut outcome,
                SearchHit {
                    kind: SearchKind::Metadata,
                    document: Some(metadata.document.clone()),
                    entity: metadata.entity,
                    entity_name: metadata.entity_name.clone(),
                    field: metadata.key.clone(),
                    detail: metadata.value.clone(),
                    path: None,
                    line: None,
                    target,
                },
            );
        }
        for entity in &self.entities {
            if !matches_terms(&entity.name, &terms) {
                continue;
            }
            record(
                &mut outcome,
                SearchHit {
                    kind: SearchKind::Entity,
                    document: Some(entity.document.clone()),
                    entity: Some(entity.id),
                    entity_name: entity.name.clone(),
                    field: String::new(),
                    detail: String::new(),
                    path: None,
                    line: None,
                    target: SearchTarget::Entity {
                        document: entity.document.clone(),
                        entity: entity.id,
                    },
                },
            );
        }
        for document in &self.documents {
            if !matches_terms(document, &terms) {
                continue;
            }
            record(
                &mut outcome,
                SearchHit {
                    kind: SearchKind::Document,
                    document: Some(document.clone()),
                    entity: None,
                    entity_name: String::new(),
                    field: String::new(),
                    detail: String::new(),
                    path: None,
                    line: None,
                    target: SearchTarget::Document {
                        document: document.clone(),
                    },
                },
            );
        }
        for resource in &self.resources {
            if !matches_terms(resource, &terms) {
                continue;
            }
            record(
                &mut outcome,
                SearchHit {
                    kind: SearchKind::Resource,
                    document: None,
                    entity: None,
                    entity_name: String::new(),
                    field: String::new(),
                    detail: String::new(),
                    path: Some(resource.clone()),
                    line: None,
                    target: SearchTarget::Resource {
                        path: resource.clone(),
                    },
                },
            );
        }
        for file in &self.lua {
            // 只看行文本，不把路径拼进 haystack：否则搜 `hero.lua` 会把这个文件的**每一行**
            // 都当成命中铺满清单。按文件名找脚本是「资源」那一路的职责（路径本来就进索引）。
            for (index, line) in file.lines.iter().enumerate() {
                if !matches_terms(line, &terms) {
                    continue;
                }
                let line_number = index as u32 + 1;
                record(
                    &mut outcome,
                    SearchHit {
                        kind: SearchKind::Script,
                        document: None,
                        entity: None,
                        entity_name: String::new(),
                        field: String::new(),
                        detail: truncate(line, 160),
                        path: Some(file.path.clone()),
                        line: Some(line_number),
                        target: SearchTarget::Script {
                            path: file.path.clone(),
                            line: line_number,
                        },
                    },
                );
            }
        }
        outcome
    }
}

fn record(outcome: &mut SearchOutcome, hit: SearchHit) {
    outcome.total += 1;
    if outcome.hits.len() < MAX_SEARCH_HITS {
        outcome.hits.push(hit);
    }
}

fn matches_terms(haystack: &str, terms: &[String]) -> bool {
    let lower = haystack.to_lowercase();
    terms.iter().all(|term| lower.contains(term.as_str()))
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// 手写面板组件的组件串 → `(键, 值)`。转义按引擎的 `split_fields` 口径切分。
fn raw_component_fields(payload: &str) -> Vec<(String, String)> {
    rustlua_ecs::component_schema::split_fields(payload)
        .into_iter()
        .filter_map(|segment| {
            let segment = segment.trim();
            let (key, value) = segment.split_once('=')?;
            let key = key.trim();
            (!key.is_empty()).then(|| (key.to_owned(), value.trim().to_owned()))
        })
        .collect()
}

fn collect_lua_files(directory: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 16 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if path.is_dir() {
            if matches!(
                name.as_str(),
                "builds" | "target" | ".git" | ".vs" | "node_modules" | "artifacts" | ".cache"
            ) {
                continue;
            }
            collect_lua_files(&path, out, depth + 1);
            continue;
        }
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lua"))
        {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 文档标签一律是工程内相对路径 + 正斜杠：搜索行的可读性与断链清单同口径。
    #[test]
    fn relative_text_uses_forward_slashes() {
        let root = Path::new("C:/proj");
        assert_eq!(
            relative_text(root, Path::new("C:/proj/assets/a.png")),
            "assets/a.png"
        );
        // 根外的路径原样给出（不 panic、也不假装是相对路径）。
        assert_eq!(
            relative_text(root, Path::new("D:/other/a.png")),
            "D:/other/a.png"
        );
    }
}
