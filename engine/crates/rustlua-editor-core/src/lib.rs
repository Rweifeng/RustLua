//! 编辑器无界面领域层：工程模型、场景文档、历史/补丁、选择、布局设置、资源与 schema 操作。
//!
//! 这层刻意不依赖 egui / eframe / wgpu，也不认识编辑器 shell（`EditorApp`）：
//! 面板、命令路由、预览会话留在 `editor`，两边的契约就是本 crate 的公开类型。
//! 这样无头工具（CLI/测试）可以只编译这一层，而不必拉起整套 GUI 依赖。

mod project;

pub use project::{
    DirectoryStatus, GAME_PROJECT_OPTIONAL_DIRECTORIES, GAME_PROJECT_REQUIRED_DIRECTORIES,
    ProjectError, ProjectInfo, REQUIRED_DIRECTORIES, check_project, open_project,
};

pub mod anim_ops;
pub mod diagnostics;
pub mod document;
pub mod document_space;
pub mod file_watcher;
pub mod graph_ops;
pub mod history;
pub mod inspector_state;
pub mod layout;
pub mod material_ops;
pub mod patch_history;
pub mod perf_fixture;
pub mod play_control;
pub mod prefab_graph;
pub mod recovery;
pub mod resource_browser_state;
pub mod resource_ops;
pub mod resource_refresh;
pub mod scene_interaction;
pub mod scene_tools;
pub mod scene_tree_state;
pub mod schema_audit;
pub mod selection;
pub mod services;
pub mod user_config;

pub use user_config::UserConfigStore;

pub use play_control::{
    FramePlan, MAX_CATCH_UP_STEPS, PlayControlError, PlayController, PlayState, STEPS_PER_SECOND,
};

pub use layout::{
    AssistantLayout, EditorLayout, LAYOUT_VERSION, LayoutLoad, PANEL_ASSISTANT, PANEL_CONSOLE,
    PANEL_GAME_VIEW, PANEL_GRAPH, PANEL_HIERARCHY, PANEL_INSPECTOR, PANEL_RESOURCE,
    PANEL_SCENE_VIEW, PANEL_TIMELINE, PANEL_UI_STUDIO, layout_path, load_layout, reset_layout,
    save_layout,
};

pub use file_watcher::{
    FileSignature, FileWatcher, Snapshot, WatchDiff, WatchError, WatchHandle, WatchScope,
    WatchStats, diff_snapshots, scan_tree, spawn_watcher,
};
pub use schema_audit::{SchemaAudit, SchemaFinding, audit_project};

pub use resource_ops::{
    RewriteReport, normalize_relative, paths_match, project_scene_documents,
    rewrite_component_reference, rewrite_project_references, rewrite_scene_references,
    strip_verbatim_prefix,
};

pub use prefab_graph::{MAX_NESTING_DEPTH, PrefabNesting, scan_prefab_nesting};

pub use diagnostics::{DiagnosticLocation, parse_location};
pub use perf_fixture::{
    FixtureSize, FixtureSpec, SceneFixture, build_console_fixture, build_scene_fixture,
};
pub use resource_refresh::{
    RefreshHandle, RefreshPoll, RefreshRequest, RefreshResult, spawn_refresh,
};

pub use services::{
    CancellationToken, CommandResult, CommandService, Diagnostic, DiagnosticBuffer,
    DiagnosticLevel, EditorCommand, JobError, JobProgress, JobReport, SelectionService,
    WorkspaceController, WorkspaceService, run_cancellable_job,
};

pub use resource_browser_state::{ResourceBrowserError, ResourceBrowserState, ResourceEntry};

pub use selection::{SelectionModel, SelectionSource};

pub use inspector_state::{
    Collider3dFields, ContainerFields, ContainerPreset, InspectorError, InspectorSnapshot,
    InspectorState, LayoutItemFields, TransformFields,
};

pub use scene_tree_state::{ReparentPosition, SceneTreeError, SceneTreeState};

pub use document::{DiskStatus, DocumentError, PlaySession, ProjectDocument, SceneDocument};

pub use history::{
    CommandHistory, CommandKind, EditCommand, EditTransactionState, HistoryError, TransactionError,
};

pub use patch_history::{EditPatch, FieldChange, PatchHistory, scene_field_patch};

pub use recovery::{
    AutoSaveController, RecoveryCandidate, RecoveryError, RecoveryIssue, RecoveryManager,
    RecoveryScan,
};

/// 场景工具枚举只有一处定义（交互词汇表），`scene_tools` 的能力表也指向它。
pub use scene_interaction::SceneTool;

/// 测试用唯一令牌：进程 ID + 原子计数 + 时间戳。
///
/// 只取时间戳是不够的——Windows 的时钟粒度下，并行运行的测试可能拿到**同一个**
/// 纳秒值，于是临时目录互撞（`opens_project_and_reports_required_directories`
/// 曾因此在一次全量测试里偶发失败：另一个测试先在同一目录写了 `project.toml`，
/// 于是它被识别成独立游戏工程，目录数断言随之失败）。
#[cfg(test)]
pub(crate) fn test_token() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = u128::from(COUNTER.fetch_add(1, Ordering::Relaxed));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    (u128::from(std::process::id()) << 96) ^ (counter << 64) ^ (nanos & 0xFFFF_FFFF_FFFF_FFFF)
}
