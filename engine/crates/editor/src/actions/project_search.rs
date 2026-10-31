//! 全工程搜索的 shell 侧：建索引、跑查询、把双击的一行落到具体位置。
//!
//! 面板只发这四个入口（建索引 / 跑查询 / 定位一行 / 处理挂起的定位），扫描与跳转语义
//! 都在这里——与断链检查窗同一条口径：**读盘只在显式动作时发生**。
//!
//! 三条跳转语义（对应索引的三类来源）：
//! 1. **编辑文档里的字段/实体/文档**：先保证那份文档是当前的（可能要换文档，换文档要走
//!    E06 的脏闸门），再选中实体、把 Inspector 的字段过滤词钉在那一行上。
//! 2. **资源**：交给资源面板的既有「定位」链路（`request_resource_locate`），与属性面板
//!    那个「定位」按钮**同一条语义**——不另造一套选中逻辑。
//! 3. **Lua 文本**：交给只读查看器（`open_lua_source`）并带上行号。
//!
//! 一条守卫：**播放模式下不跳去改编辑文档的选择**。运行态镜像与编辑文档是两份东西，
//! 在 Play 里"跳过去"会把选区落在看不见的文档上；只读的查看脚本与定位资源不受影响。

use crate::EditorApp;
use crate::WorkspaceTab;
use crate::actions::PendingChange;
use crate::actions::project_index::{ProjectIndex, SearchTarget, relative_text};
use editor::{CommandResult, EditorCommand};
use std::path::{Path, PathBuf};

impl EditorApp {
    /// 打开搜索窗：还没有索引（或索引属于别的工程）就先建一次。
    pub(crate) fn open_project_search(&mut self) {
        if !self.project_loaded {
            self.status = "全工程搜索需要先打开工程".to_owned();
            return;
        }
        self.project_search.show = true;
        self.ensure_project_search_index();
        self.run_project_search();
    }

    /// 从别处（资源面板右键）带着查询词打开：用于"这个资源谁在用"的反查入口。
    pub(crate) fn open_project_search_for(&mut self, query: String) {
        if !self.project_loaded {
            self.status = "全工程搜索需要先打开工程".to_owned();
            return;
        }
        self.project_search.show = true;
        self.project_search.query = query;
        self.ensure_project_search_index();
        self.run_project_search();
    }

    /// 索引是快照：只在没有索引、或它属于另一份工程时重建（换工程的代价不该由每次搜索付）。
    pub(crate) fn ensure_project_search_index(&mut self) {
        let stale = self
            .project_search
            .index
            .as_ref()
            .is_none_or(|index| !index.belongs_to(&self.project.root));
        if stale {
            self.rebuild_project_search_index();
        }
    }

    /// 显式重建索引（面板上的「重建索引」）：读全部文档 + 脚本，然后按当前查询词再搜一次。
    pub(crate) fn rebuild_project_search_index(&mut self) {
        if !self.project_loaded {
            self.status = "建立工程索引需要先打开工程".to_owned();
            return;
        }
        let mut resources: Vec<String> = self
            .resources
            .entries()
            .iter()
            .map(editor::ResourceEntry::path_string)
            .collect();
        resources.sort();
        resources.dedup();
        let index = ProjectIndex::scan(
            &self.project.root,
            self.document.current_scene.as_ref(),
            resources,
        );
        let mut message = format!(
            "索引：{} 份文档 · {} 个脚本 · {} 条资源路径",
            index.scanned_documents,
            index.scanned_lua_files,
            index.resources.len()
        );
        if index.resources.is_empty() {
            message.push_str("（资源索引还没建立：刷新资源后才能在文档里按路径反查资源）");
        }
        if !index.unreadable.is_empty() {
            message.push_str(&format!(
                "；{} 份文档读不动：{}",
                index.unreadable.len(),
                index.unreadable.join("、")
            ));
        }
        if !index.unreadable_scripts.is_empty() {
            message.push_str(&format!(
                "；{} 个脚本读不动（多数不是 UTF-8）：{}",
                index.unreadable_scripts.len(),
                index.unreadable_scripts.join("、")
            ));
        }
        self.project_search.index = Some(index);
        self.project_search.message = message;
        self.run_project_search();
    }

    /// 只读快照跑一次查询：不读盘，所以每次按键都能跑。
    pub(crate) fn run_project_search(&mut self) {
        let query = self.project_search.query.clone();
        let outcome = match self.project_search.index.as_ref() {
            Some(index) => index.search(&query),
            None => Default::default(),
        };
        let truncated = outcome.total > outcome.hits.len();
        self.project_search.total_hits = outcome.total;
        self.project_search.hits = outcome.hits;
        self.project_search.selected = 0;
        self.project_search.truncated = truncated;
    }

    /// 双击/回车一行：把它落到具体位置（换文档时可能先挂起，见 `tick_pending_search_jump`）。
    pub(crate) fn activate_search_hit(&mut self, index: usize) -> CommandResult {
        let Some(target) = self
            .project_search
            .hits
            .get(index)
            .map(|hit| hit.target.clone())
        else {
            return CommandResult::NoOp;
        };
        self.activate_search_target(&target)
    }

    /// 落一条搜索结果。
    ///
    /// 目标文档不是当前文档时走 E06 的闸门：clean 就直接打开并继续定位，dirty 就挂起
    /// （用户选完「保存/放弃/取消」再由 `tick_pending_search_jump` 补做定位）。
    pub(crate) fn activate_search_target(&mut self, target: &SearchTarget) -> CommandResult {
        if let Some(document) = target.document() {
            if self.is_playing() {
                self.status = "播放模式下不能切换编辑文档，请先点击停止".to_owned();
                return CommandResult::Rejected(self.status.clone());
            }
            if !self.current_document_is(document) {
                let opened = self.request_open_for_search(target);
                if !opened {
                    return CommandResult::Applied;
                }
            }
        }
        match self.apply_search_target(target) {
            Ok(status) => {
                self.status = status;
                CommandResult::Applied
            }
            Err(reason) => CommandResult::Rejected(reason),
        }
    }

    /// 下一帧：挂起的定位是否已经可以落地。
    ///
    /// 三种结局都要认：文档真的打开了（补做定位）、用户取消或打开失败（**明确作废**，
    /// 不能留着下一轮误跳）、确认窗还挂着（这帧什么都不做）。
    pub(crate) fn tick_pending_search_jump(&mut self) {
        let Some(target) = self.project_search.pending_jump.clone() else {
            return;
        };
        if self.pending_document_change.is_some() {
            return;
        }
        self.project_search.pending_jump = None;
        let Some(document) = target.document() else {
            return;
        };
        if !self.current_document_is(document) {
            self.status = format!("已取消定位：当前文档不是 {document}");
            return;
        }
        match self.apply_search_target(&target) {
            Ok(status) => self.status = status,
            Err(reason) => self.status = reason,
        }
    }

    /// 当前编辑文档是不是 `document`（工程内相对路径，正斜杠；未保存场景用同一个标签）。
    pub(crate) fn current_document_is(&self, document: &str) -> bool {
        self.current_document_label().as_deref() == Some(document)
    }

    /// 当前编辑文档的标签（与索引里那份同一写法）。
    fn current_document_label(&self) -> Option<String> {
        let document = self.document.current_scene.as_ref()?;
        Some(match document.path.as_ref() {
            Some(path) => relative_text(&self.project.root, path),
            None => "未命名场景".to_owned(),
        })
    }

    /// 为定位打开另一份文档。返回 `true` = 现在已经是当前文档，可以继续定位。
    fn request_open_for_search(&mut self, target: &SearchTarget) -> bool {
        let Some(document) = target.document().map(str::to_owned) else {
            return true;
        };
        if self.pending_document_change.is_some() {
            self.status = "已有一项待确认的文档切换，先处理它再定位".to_owned();
            return false;
        }
        self.request_document_change(PendingChange::OpenResource {
            relative_path: PathBuf::from(&document),
        });
        if self.current_document_is(&document) {
            return true;
        }
        if self.pending_document_change.is_some() {
            // dirty：把这一次定位挂起，等确认窗裁决（见 `tick_pending_search_jump`）。
            self.project_search.pending_jump = Some(target.clone());
            self.status = format!("定位需要先打开 {document}（当前文档有未保存的改动）");
            return false;
        }
        // 闸门没放行也没挂起（预制体编辑上下文、打开失败）：原因已经在 `self.status` 里。
        false
    }

    /// 目标文档已经在当前位后，真正改选区/过滤词/其他窗口的落点。
    fn apply_search_target(&mut self, target: &SearchTarget) -> Result<String, String> {
        match target {
            SearchTarget::Field {
                document,
                entity,
                field_path,
                schema_field,
            } => {
                self.select_entity_for_search(*entity)?;
                self.show_inspector_panel = true;
                self.focus_workspace_tab(WorkspaceTab::Scene);
                let located = format!("{document} ▸ E#{} ▸ {field_path}", entity.0);
                if !*schema_field {
                    self.inspector_field_filter.clear();
                    return Ok(format!(
                        "已选中实体：{located}（{field_path} 由手写面板显示，通用字段过滤词已清空）"
                    ));
                }
                // 精确判据：过滤词必须真的把那一行留在结果里，否则宁可不填
                // （填一个搜不到东西的词，比不填更容易让人以为"已经定位到了"）。
                let rows = editor::filter_field_rows(&self.schema_field_rows(), field_path);
                if rows.iter().any(|row| row.path == *field_path) {
                    self.inspector_field_filter = field_path.clone();
                    Ok(format!("已定位字段：{located}"))
                } else {
                    self.inspector_field_filter.clear();
                    Ok(format!(
                        "已选中实体：{located}（该字段不在当前实体的通用字段行里，过滤词已清空）"
                    ))
                }
            }
            SearchTarget::Entity { document, entity } => {
                self.select_entity_for_search(*entity)?;
                self.show_inspector_panel = true;
                self.focus_workspace_tab(WorkspaceTab::Scene);
                self.inspector_field_filter.clear();
                Ok(format!(
                    "已选中实体：{document} ▸ E#{} ▸ {}",
                    entity.0, self.entity_name
                ))
            }
            SearchTarget::Document { document } => Ok(format!("已打开文档：{document}")),
            SearchTarget::Resource { path } => {
                self.show_resources_panel = true;
                self.request_resource_locate(path);
                if self.pending_resource_locate.is_some() {
                    Ok(format!("已在资源面板定位：{path}"))
                } else {
                    Err(self.status.clone())
                }
            }
            SearchTarget::Script { path, line } => {
                self.open_lua_source(Path::new(path), Some(*line));
                Ok(format!("已在只读查看器打开 {path} 第 {line} 行"))
            }
        }
    }

    /// 选一个实体（与层级点一行同一入口）；不在当前文档里就报出来。
    fn select_entity_for_search(
        &mut self,
        entity: rustlua_scene::scene::SceneEntityId,
    ) -> Result<(), String> {
        match self.dispatch(EditorCommand::SelectEntity { id: entity }) {
            CommandResult::Applied | CommandResult::NoOp => Ok(()),
            CommandResult::Rejected(reason) => Err(reason),
        }
    }
}
