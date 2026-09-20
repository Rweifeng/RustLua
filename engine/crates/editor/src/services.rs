//! Editor service boundaries used by panels and the application shell.
//!
//! These services intentionally wrap the existing document/history models.  The first
//! migration step is an explicit boundary with typed inputs and immutable task results;
//! callers can move behind it without changing the on-disk scene format.

use crate::{
    CommandHistory, CommandKind, DocumentError, PlaySession, ProjectDocument, ProjectInfo,
    SceneDocument, SelectionModel, SelectionSource,
};
use engine::core::scene::SceneEntityId;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

/// Typed document operations emitted by panels and shortcuts.
///
/// Views emit these instead of calling document-mutating methods directly (docs/55 §4.1
/// rule 1). Every variant is routed through one dispatch point that owns the commit
/// boundaries, so the same action behaves identically from a menu, a shortcut or a panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorCommand {
    NewScene {
        name: String,
    },
    OpenDocument {
        path: PathBuf,
    },
    Save,
    SaveAs {
        path: PathBuf,
    },
    Undo,
    Redo,
    StartPlay,
    StopPlay,
    /// ER-10：运行中暂停（`Playing` → `Paused`），逻辑时间冻结。
    PausePlay,
    /// ER-10：暂停后继续运行（`Paused` → `Playing`）。
    ResumePlay,
    /// ER-10：暂停态下单帧步进 `frames` 个逻辑帧（保持暂停语义）。
    StepPlay {
        frames: u32,
    },
    /// ER-10：重启运行（释放当前会话后按编辑器快照重新开始）。
    RestartPlay,
    /// Create an entity (optionally as a child of `parent`, optionally with a 3D transform).
    CreateEntity {
        name: String,
        parent: Option<SceneEntityId>,
        transform_3d: bool,
    },
    /// Duplicate the current selection in place (one undo entry).
    DuplicateSelection,
    /// Delete the current selection (one undo entry for the batch).
    DeleteSelection,
    /// Move the current selection under `target`; `None` means "to scene root".
    ReparentSelection {
        target: Option<SceneEntityId>,
        position: crate::ReparentPosition,
    },
    /// Select one entity (selection is the single source of truth, so this is a command too).
    SelectEntity {
        id: SceneEntityId,
    },
    /// docs/55 阶段 2：按 schema/PropertyPath 改一个组件字段（一次提交 = 一条 Undo）。
    ///
    /// 通用（schema 驱动）Inspector 只发这个 action，不直接写组件串；取值校验、
    /// "无变化不产生历史"、Play 锁定都在 shell 的 dispatch 里统一处理。
    SetComponentField {
        entity: SceneEntityId,
        component: String,
        field: String,
        value: String,
    },
    /// Prefab 实例的字段级覆盖：把实例取值写回源 Prefab 并更新全部实例。
    ApplyPrefabOverride {
        entity: SceneEntityId,
        field: String,
    },
    /// Prefab 实例的字段级覆盖：把该字段还原为源 Prefab 的取值。
    RevertPrefabOverride {
        entity: SceneEntityId,
        field: String,
    },
    /// 重新应用源 Prefab（重建实例，保留已记录的覆盖）。
    ReapplyPrefab {
        entity: SceneEntityId,
    },
    /// 把实例写回源 Prefab 文件（并更新全部实例）。
    SavePrefabToSource {
        entity: SceneEntityId,
    },
    /// 解包：断开实例与源 Prefab 的关系，实体留在场景里。
    UnpackPrefab {
        entity: SceneEntityId,
    },
    /// 回退实例上的**全部**覆盖（用源 Prefab 重建实例）。
    RevertPrefabOverrides {
        entity: SceneEntityId,
    },
}

/// Result of a command, suitable for status bars and notifications.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandResult {
    Applied,
    NoOp,
    Rejected(String),
}

/// Single gateway for the currently open workspace document.
#[derive(Debug)]
pub struct WorkspaceService {
    document: ProjectDocument,
    play_session: Option<PlaySession>,
}

impl WorkspaceService {
    pub fn open(project: ProjectInfo) -> Self {
        Self {
            document: ProjectDocument::open(project),
            play_session: None,
        }
    }

    pub fn project(&self) -> &ProjectInfo {
        &self.document.project
    }

    pub fn document(&self) -> &ProjectDocument {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut ProjectDocument {
        &mut self.document
    }

    pub fn current_scene(&self) -> Option<&SceneDocument> {
        self.document.current_scene.as_ref()
    }

    pub fn current_scene_mut(&mut self) -> Option<&mut SceneDocument> {
        self.document.current_scene.as_mut()
    }

    pub fn is_dirty(&self) -> bool {
        self.current_scene().is_some_and(SceneDocument::is_dirty)
    }

    pub fn open_scene(&mut self, path: impl AsRef<Path>) -> Result<(), DocumentError> {
        self.document.open_scene(path)
    }

    pub fn new_scene(&mut self, name: impl Into<String>) {
        self.document.new_scene(name);
    }

    pub fn save(&mut self) -> Result<(), DocumentError> {
        self.document.save_scene()
    }

    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<(), DocumentError> {
        self.document.save_scene_as(path)
    }

    pub fn start_play(&mut self) -> Result<(), &'static str> {
        if self.play_session.is_some() {
            return Err("play session already active");
        }
        let scene = self.current_scene().ok_or("no scene is open")?;
        self.play_session = Some(PlaySession::start(scene));
        Ok(())
    }

    pub fn stop_play(&mut self) -> Result<(), &'static str> {
        let session = self
            .play_session
            .take()
            .ok_or("play session is not active")?;
        self.document.current_scene = Some(session.stop());
        Ok(())
    }

    pub fn is_playing(&self) -> bool {
        self.play_session.is_some()
    }

    pub fn runtime_scene(&self) -> Option<&SceneDocument> {
        self.play_session
            .as_ref()
            .map(PlaySession::runtime_document)
    }
}

/// Command/history facade.  Snapshot history remains the compatibility implementation;
/// the facade gives future patch commands and checkpoints a stable insertion point, and it
/// owns the undo memory budget so a long session cannot grow history without limit.
#[derive(Debug)]
pub struct CommandService {
    history: CommandHistory,
    max_entries: usize,
    /// Undo entries discarded so far because they exceeded the budget.
    dropped_entries: u64,
}

/// 注意：**不能**用 `#[derive(Default)]`。派生出来的 `max_entries` 是 `0`，
/// 而 `enforce_budget` 会照此把历史全丢——"默认构造 = 没有 Undo"是个安静的陷阱。
/// 由性能基线场景 `schema_field_edit_1k` 的报告行暴露（`budget=0`）。
impl Default for CommandService {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_ENTRIES)
    }
}

impl CommandService {
    /// Default undo budget: deep enough for normal editing, bounded for long sessions.
    pub const DEFAULT_MAX_ENTRIES: usize = 200;

    pub fn new(max_entries: usize) -> Self {
        Self {
            history: CommandHistory::new(),
            max_entries: max_entries.max(1),
            dropped_entries: 0,
        }
    }

    pub fn history(&self) -> &CommandHistory {
        &self.history
    }

    pub fn history_mut(&mut self) -> &mut CommandHistory {
        &mut self.history
    }

    /// Execute a document edit with budget enforcement. Mirrors `CommandHistory::execute`
    /// (`Ok(true)` when the edit changed the document) so existing call sites keep working.
    pub fn execute<F, E>(
        &mut self,
        document: &mut SceneDocument,
        label: impl Into<String>,
        kind: CommandKind,
        edit: F,
    ) -> Result<bool, E>
    where
        F: FnOnce(&mut SceneDocument) -> Result<(), E>,
    {
        let changed = self.history.execute(document, label, kind, edit)?;
        self.enforce_budget();
        Ok(changed)
    }

    /// Service-level undo that reports an outcome instead of the command label. `undo` and
    /// `redo` themselves resolve to `CommandHistory` through `Deref`, so call sites that
    /// need the label keep working unchanged.
    pub fn undo_applied(&mut self, document: &mut SceneDocument) -> CommandResult {
        match self.history.undo(document) {
            Ok(_) => CommandResult::Applied,
            Err(_) => CommandResult::NoOp,
        }
    }

    pub fn redo_applied(&mut self, document: &mut SceneDocument) -> CommandResult {
        match self.history.redo(document) {
            Ok(_) => CommandResult::Applied,
            Err(_) => CommandResult::NoOp,
        }
    }

    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub fn dropped_entries(&self) -> u64 {
        self.dropped_entries
    }

    /// Undo entries exceeding the budget are dropped oldest-first; the count is reported so
    /// the profiler and status bar can surface history pressure.
    pub fn enforce_budget(&mut self) {
        let dropped = self.history.enforce_undo_limit(self.max_entries);
        self.dropped_entries = self.dropped_entries.saturating_add(dropped as u64);
    }
}

impl std::ops::Deref for CommandService {
    type Target = CommandHistory;

    fn deref(&self) -> &Self::Target {
        &self.history
    }
}

impl std::ops::DerefMut for CommandService {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.history
    }
}

/// State-kernel facade used by the shell. Views submit typed commands here and observe the
/// resulting immutable status instead of mutating a document directly.
#[derive(Debug)]
pub struct WorkspaceController {
    pub workspace: WorkspaceService,
    pub commands: CommandService,
    pub selection: SelectionService,
    pub diagnostics: DiagnosticBuffer,
}

impl WorkspaceController {
    pub fn open(project: ProjectInfo, history_entries: usize, diagnostic_entries: usize) -> Self {
        Self {
            workspace: WorkspaceService::open(project),
            commands: CommandService::new(history_entries),
            selection: SelectionService::new(),
            diagnostics: DiagnosticBuffer::new(diagnostic_entries),
        }
    }

    pub fn dispatch(&mut self, command: EditorCommand) -> CommandResult {
        let result = match command {
            EditorCommand::NewScene { name } => {
                self.workspace.new_scene(name);
                self.commands.history_mut().clear();
                self.selection.clear(SelectionSource::Command);
                CommandResult::Applied
            }
            EditorCommand::OpenDocument { path } => match self.workspace.open_scene(path) {
                Ok(()) => {
                    self.commands.history_mut().clear();
                    self.selection.clear(SelectionSource::Command);
                    CommandResult::Applied
                }
                Err(error) => CommandResult::Rejected(error.to_string()),
            },
            EditorCommand::Save => match self.workspace.save() {
                Ok(()) => CommandResult::Applied,
                Err(error) => CommandResult::Rejected(error.to_string()),
            },
            EditorCommand::SaveAs { path } => match self.workspace.save_as(path) {
                Ok(()) => CommandResult::Applied,
                Err(error) => CommandResult::Rejected(error.to_string()),
            },
            EditorCommand::Undo => match self.workspace.current_scene_mut() {
                Some(document) => self.commands.undo_applied(document),
                None => CommandResult::NoOp,
            },
            EditorCommand::Redo => match self.workspace.current_scene_mut() {
                Some(document) => self.commands.redo_applied(document),
                None => CommandResult::NoOp,
            },
            EditorCommand::StartPlay => self.workspace.start_play().map_or_else(
                |error| CommandResult::Rejected(error.into()),
                |_| CommandResult::Applied,
            ),
            EditorCommand::StopPlay => self.workspace.stop_play().map_or_else(
                |error| CommandResult::Rejected(error.into()),
                |_| {
                    self.selection.clear(SelectionSource::Playback);
                    CommandResult::Applied
                },
            ),
            // 实体级命令（创建/复制/删除/换父/选中）需要编辑器会话状态
            // （名称缓冲、Inspector 同步、场景视图缓存失效），由 shell 的
            // `EditorApp::dispatch` 处理；服务层显式拒绝而不是静默忽略。
            EditorCommand::CreateEntity { .. }
            | EditorCommand::DuplicateSelection
            | EditorCommand::DeleteSelection
            | EditorCommand::ReparentSelection { .. }
            | EditorCommand::SelectEntity { .. }
            | EditorCommand::SetComponentField { .. }
            | EditorCommand::ApplyPrefabOverride { .. }
            | EditorCommand::RevertPrefabOverride { .. }
            | EditorCommand::ReapplyPrefab { .. }
            | EditorCommand::SavePrefabToSource { .. }
            | EditorCommand::UnpackPrefab { .. }
            | EditorCommand::RevertPrefabOverrides { .. } => CommandResult::Rejected(
                "实体级命令需要编辑器会话（请在 shell 的 dispatch 入口执行）".to_owned(),
            ),
            // 运行控制命令（ER-10 暂停/继续/步进/重启）由编辑器会话持有的
            // `PlayController` 驱动；服务层同样显式拒绝。
            EditorCommand::PausePlay
            | EditorCommand::ResumePlay
            | EditorCommand::StepPlay { .. }
            | EditorCommand::RestartPlay => CommandResult::Rejected(
                "运行控制命令需要编辑器会话（请在 shell 的 dispatch 入口执行）".to_owned(),
            ),
        };
        if let CommandResult::Rejected(message) = &result {
            self.diagnostics
                .push(Diagnostic::new(DiagnosticLevel::Error, message.clone()));
        }
        result
    }
}

/// Explicit owner for the one authoritative editor selection.
#[derive(Clone, Debug, Default)]
pub struct SelectionService {
    model: SelectionModel,
}

impl SelectionService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn model(&self) -> &SelectionModel {
        &self.model
    }

    pub fn model_mut(&mut self) -> &mut SelectionModel {
        &mut self.model
    }

    pub fn select(&mut self, id: SceneEntityId, source: SelectionSource) {
        self.model.set_single(id, source);
    }

    pub fn clear(&mut self, source: SelectionSource) {
        self.model.clear(source);
    }

    pub fn retain_valid(&mut self, valid: impl Fn(SceneEntityId) -> bool) {
        self.model.retain_valid(valid);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
    pub source: Option<String>,
    pub path: Option<PathBuf>,
    pub line: Option<u32>,
    pub entity: Option<SceneEntityId>,
    pub timestamp: SystemTime,
}

impl Diagnostic {
    pub fn new(level: DiagnosticLevel, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
            source: None,
            path: None,
            line: None,
            entity: None,
            timestamp: SystemTime::now(),
        }
    }
}

/// Bounded diagnostic buffer.  Appending never grows memory without limit and filtering
/// can be performed incrementally by the UI from a stable snapshot.
#[derive(Clone, Debug)]
pub struct DiagnosticBuffer {
    capacity: usize,
    entries: VecDeque<Diagnostic>,
}

impl DiagnosticBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::with_capacity(capacity.max(1)),
        }
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(diagnostic);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Diagnostic> {
        self.entries.iter()
    }

    pub fn snapshot(&self) -> Vec<Diagnostic> {
        self.entries.iter().cloned().collect()
    }
}

/// Cooperative cancellation token for filesystem/import/validation jobs.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn check(&self) -> Result<(), JobError> {
        if self.is_cancelled() {
            Err(JobError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobError {
    Cancelled,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct JobProgress {
    pub completed: usize,
    pub total: Option<usize>,
    pub message: String,
}

impl JobProgress {
    pub fn fraction(&self) -> Option<f32> {
        self.total
            .filter(|total| *total > 0)
            .map(|total| (self.completed as f32 / total as f32).clamp(0.0, 1.0))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobReport {
    pub name: String,
    pub elapsed: Duration,
    pub cancelled: bool,
    pub diagnostics: Vec<Diagnostic>,
}

/// Small helper for synchronous job implementations and tests; production callers can run
/// the closure on their executor and use the same token/report contract.
pub fn run_cancellable_job<F>(
    name: impl Into<String>,
    token: &CancellationToken,
    job: F,
) -> Result<JobReport, JobError>
where
    F: FnOnce(&CancellationToken, &mut DiagnosticBuffer) -> Result<(), JobError>,
{
    let started = Instant::now();
    let mut diagnostics = DiagnosticBuffer::new(256);
    job(token, &mut diagnostics)?;
    token.check()?;
    Ok(JobReport {
        name: name.into(),
        elapsed: started.elapsed(),
        cancelled: false,
        diagnostics: diagnostics.snapshot(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DirectoryStatus, ProjectInfo};

    fn project() -> ProjectInfo {
        ProjectInfo {
            root: PathBuf::from("project"),
            version: "test".into(),
            directories: vec![DirectoryStatus {
                name: "scenes".into(),
                exists: true,
                is_directory: true,
            }],
        }
    }

    #[test]
    fn workspace_service_keeps_play_runtime_isolated() {        let mut workspace = WorkspaceService::open(project());
        workspace.new_scene("main");
        workspace.current_scene_mut().unwrap().scene.name = "editor".into();
        workspace.start_play().unwrap();
        assert!(workspace.is_playing());
        workspace
            .play_session
            .as_mut()
            .unwrap()
            .runtime_document_mut()
            .scene
            .name = "runtime".into();
        assert_eq!(workspace.current_scene().unwrap().scene.name, "editor");
        workspace.stop_play().unwrap();
        assert_eq!(workspace.current_scene().unwrap().scene.name, "editor");
    }

    #[test]
    fn diagnostics_are_bounded_fifo() {
        let mut buffer = DiagnosticBuffer::new(2);
        buffer.push(Diagnostic::new(DiagnosticLevel::Info, "one"));
        buffer.push(Diagnostic::new(DiagnosticLevel::Warning, "two"));
        buffer.push(Diagnostic::new(DiagnosticLevel::Error, "three"));
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.snapshot()[0].message, "two");
    }

    #[test]
    fn cancellation_is_cooperative() {
        let token = CancellationToken::new();
        token.cancel();
        let result = run_cancellable_job("scan", &token, |token, _| token.check());
        assert_eq!(result, Err(JobError::Cancelled));
    }

    /// `CommandService::default()` 必须是**有预算**的 200，而不是派生出来的 0。
    ///
    /// 派生的 `Default` 会让 `max_entries = 0`，于是每次编辑后 `enforce_budget`
    /// 把整条历史丢掉——"默认构造 = 没有 Undo"。这条测试就是钉住这个陷阱。
    #[test]
    fn default_command_service_keeps_the_documented_undo_budget() {
        let service = CommandService::default();
        assert_eq!(service.max_entries(), CommandService::DEFAULT_MAX_ENTRIES);
        assert_eq!(service.max_entries(), 200);
        assert_eq!(service.dropped_entries(), 0);
    }
}
