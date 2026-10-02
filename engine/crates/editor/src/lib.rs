pub mod diagnostics;
pub mod file_watcher;
pub mod layout;
pub mod perf_fixture;
pub mod play_control;
pub mod prefab_graph;
pub mod schema_audit;

pub use play_control::{
    FramePlan, MAX_CATCH_UP_STEPS, PlayControlError, PlayController, PlayState, STEPS_PER_SECOND,
};

pub use layout::{
    EditorLayout, LAYOUT_VERSION, LayoutLoad, PANEL_CONSOLE, PANEL_GAME_VIEW, PANEL_HIERARCHY,
    PANEL_INSPECTOR, PANEL_RESOURCE, PANEL_SCENE_VIEW, layout_path, load_layout, reset_layout,
    save_layout,
};

pub use file_watcher::{
    FileSignature, FileWatcher, Snapshot, WatchDiff, WatchError, WatchHandle, WatchScope,
    WatchStats, diff_snapshots, scan_tree, spawn_watcher,
};
pub use schema_audit::{SchemaAudit, SchemaFinding, audit_project};
pub mod profiler;
pub mod resource_browser_state;
pub mod resource_ops;
pub mod resource_refresh;
pub mod schema_fields;
pub mod services;

pub use resource_ops::{
    RewriteReport, normalize_relative, paths_match, rewrite_component_reference,
    rewrite_project_references, rewrite_scene_references, strip_verbatim_prefix,
};

pub use prefab_graph::{MAX_NESTING_DEPTH, PrefabNesting, scan_prefab_nesting};

pub use schema_fields::{
    ChoiceSource, FieldEdit, FieldUnit, HANDWRITTEN_PANEL_COMPONENTS, SORTING_LAYERS,
    SchemaComponentAction, SchemaComponentNote, SchemaComponentToggle, SchemaFieldError,
    SchemaFieldRow, component_actions, component_notes, component_toggles, driven_components,
    edit_component_field, edit_component_payload, field_choice_source, field_unit,
    filter_field_rows, is_schema_driven, mark_missing_assets, mirror_fields, picker_candidates,
    preview_field_edit, rows_for_component, rows_for_entity, rows_for_selection,
};

pub use diagnostics::{DiagnosticLocation, parse_location};
pub use perf_fixture::{
    FixtureSize, FixtureSpec, SceneFixture, build_console_fixture, build_scene_fixture,
};
pub use profiler::{
    DEFAULT_FRAME_BUDGET_MS, EditorProfiler, Percentiles, ProfilerReport, TimingSummary,
    percentiles_ms,
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

pub mod selection;

pub use selection::{SelectionModel, SelectionSource};

pub use engine::core::resource_kind::ResourceKind;

pub mod inspector_state;

pub use inspector_state::{
    Collider3dFields, ContainerFields, InspectorError, InspectorSnapshot, InspectorState,
    TransformFields,
};

pub mod scene_tree_state;

pub use scene_tree_state::{ReparentPosition, SceneTreeError, SceneTreeState};

pub mod document;

pub use document::{DiskStatus, DocumentError, PlaySession, ProjectDocument, SceneDocument};

/// docs/54 §8-⑦：编辑态的空间语义（文档世界变换、目标世界→新局部、换父 keep-world）。
pub mod document_space;

use std::{
    fs,
    path::{Path, PathBuf},
};

pub const REQUIRED_DIRECTORIES: &[&str] = &["engine", "game", "assets", "docs"];
pub const GAME_PROJECT_REQUIRED_DIRECTORIES: &[&str] = &["scripts"];
pub const GAME_PROJECT_OPTIONAL_DIRECTORIES: &[&str] = &["scenes", "assets"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub root: PathBuf,
    pub version: String,
    pub directories: Vec<DirectoryStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryStatus {
    pub name: String,
    pub exists: bool,
    pub is_directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectError {
    #[error("项目路径不存在：{}", .0.display())]
    NotFound(PathBuf),
    #[error("项目路径不是目录：{}", .0.display())]
    NotDirectory(PathBuf),
    #[error("项目缺少必需目录：{name}（项目根目录：{}）", root.display())]
    MissingDirectory { root: PathBuf, name: String },
    #[error("读取项目路径失败：{}：{message}", path.display())]
    Io { path: PathBuf, message: String },
}

pub fn open_project(path: impl AsRef<Path>) -> Result<ProjectInfo, ProjectError> {
    let requested = path.as_ref();
    let root = fs::canonicalize(requested).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ProjectError::NotFound(requested.to_path_buf())
        } else {
            ProjectError::Io {
                path: requested.to_path_buf(),
                message: error.to_string(),
            }
        }
    })?;
    let metadata = fs::metadata(&root).map_err(|error| ProjectError::Io {
        path: root.clone(),
        message: error.to_string(),
    })?;
    if !metadata.is_dir() {
        return Err(ProjectError::NotDirectory(root));
    }

    let standalone_game = root.join("project.toml").is_file();
    let directory_names: &[&str] = if standalone_game {
        GAME_PROJECT_REQUIRED_DIRECTORIES
    } else {
        REQUIRED_DIRECTORIES
    };
    let mut directories =
        Vec::with_capacity(directory_names.len() + GAME_PROJECT_OPTIONAL_DIRECTORIES.len());
    for name in directory_names {
        let path = root.join(name);
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ProjectError::MissingDirectory {
                    root: root.clone(),
                    name: (*name).to_owned(),
                });
            }
            Err(error) => {
                return Err(ProjectError::Io {
                    path: path.clone(),
                    message: error.to_string(),
                });
            }
        };
        directories.push(DirectoryStatus {
            name: (*name).to_owned(),
            exists: true,
            is_directory: metadata.is_dir(),
        });
        if !metadata.is_dir() {
            return Err(ProjectError::MissingDirectory {
                root: root.clone(),
                name: (*name).to_owned(),
            });
        }
    }
    if standalone_game {
        for name in GAME_PROJECT_OPTIONAL_DIRECTORIES {
            let path = root.join(name);
            directories.push(DirectoryStatus {
                name: (*name).to_owned(),
                exists: path.is_dir(),
                is_directory: path.is_dir(),
            });
        }
    }

    let version = read_baseline_version(&root);
    Ok(ProjectInfo {
        root,
        version,
        directories,
    })
}

fn read_baseline_version(root: &Path) -> String {
    let mut command = std::process::Command::new("git");
    command
        .args(["describe", "--tags", "--abbrev=0"])
        .current_dir(root);
    #[cfg(windows)]
    {
        // 编辑器是 GUI 程序：git 是控制台程序，不设 CREATE_NO_WINDOW 会闪现终端窗口。
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output();
    match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        _ => "未检测到 Git 标签".to_owned(),
    }
}

pub fn check_project(path: impl AsRef<Path>) -> Result<String, ProjectError> {
    let info = open_project(path)?;
    let mut output = format!(
        "editor_project_ok root={} baseline={}\n",
        info.root.display(),
        info.version
    );
    for directory in info.directories {
        output.push_str(&format!("directory={} status=present\n", directory.name));
    }
    // 组件 schema 审计（docs/55 §1 第 3 条）：已注册组件的字段级漂移会被逐条列出，
    // 未注册组件单独计数（不是错误——扩展组件本来就可以没有 schema）。
    match schema_audit::audit_project(&info.root) {
        Ok(audit) => {
            for line in audit.report_lines() {
                output.push_str(&line);
                output.push('\n');
            }
        }
        Err(error) => output.push_str(&format!("schema_audit_failed reason={error}\n")),
    }
    Ok(output)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_project() -> PathBuf {
        // 用 crate 级唯一令牌：并行测试下也不会有两个测试拿到同一目录。
        let root =
            std::env::temp_dir().join(format!("rustlua-editor-test-{}", crate::test_token()));
        fs::create_dir_all(&root).expect("temporary project should be created");
        root
    }

    #[test]
    fn opens_project_and_reports_required_directories() {
        let root = temp_project();
        for name in REQUIRED_DIRECTORIES {
            fs::create_dir(root.join(name)).expect("required directory should be created");
        }
        let info = open_project(&root).expect("project should open");
        assert_eq!(info.directories.len(), REQUIRED_DIRECTORIES.len());
        assert!(
            check_project(&root)
                .expect("check should pass")
                .contains("editor_project_ok")
        );
        fs::remove_dir_all(root).expect("temporary project should be removed");
    }

    #[test]
    fn opens_standalone_game_project_without_engine_directories() {
        let root = temp_project();
        fs::create_dir(root.join("scripts")).expect("scripts directory should be created");
        fs::write(root.join("project.toml"), "[project]\nname = \"demo\"\n")
            .expect("manifest should be created");
        let info = open_project(&root).expect("standalone game project should open");
        assert_eq!(info.directories[0].name, "scripts");
        assert!(!info.directories.iter().any(|entry| entry.name == "engine"));
        fs::remove_dir_all(root).expect("temporary project should be removed");
    }

    #[test]
    fn rejects_missing_project() {
        let path = std::env::temp_dir().join("rustlua-editor-project-that-does-not-exist");
        assert!(matches!(open_project(path), Err(ProjectError::NotFound(_))));
    }

    #[test]
    fn rejects_project_with_missing_required_directory() {
        let root = temp_project();
        fs::create_dir(root.join("engine")).expect("directory should be created");
        assert!(matches!(
            open_project(&root),
            Err(ProjectError::MissingDirectory { .. })
        ));
        fs::remove_dir_all(root).expect("temporary project should be removed");
    }
}

pub mod history;
pub use history::{
    CommandHistory, CommandKind, EditCommand, EditTransactionState, HistoryError, TransactionError,
};

/// docs/55 ER-03「patch 原型」：把字段级编辑表示成补丁而不是整份文档快照。
pub mod patch_history;
pub use patch_history::{EditPatch, FieldChange, PatchHistory};

pub mod recovery;
pub use recovery::{
    AutoSaveController, RecoveryCandidate, RecoveryError, RecoveryIssue, RecoveryManager,
    RecoveryScan,
};
