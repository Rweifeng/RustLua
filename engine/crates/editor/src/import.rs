//! 游戏项目导入与资源导入：后台线程任务、进度回传、结果应用。

use crate::EditorApp;
use crate::panels::progress::{ProgressEvent, ProgressKind};
use crate::util::{
    display_path, format_project_error, save_recent_projects, strip_verbatim_prefix,
};
use editor::{
    ProjectDocument, ProjectInfo, RecoveryManager, RecoveryScan, ResourceBrowserState, open_project,
};
use engine::core::{
    import_cache::ImportCache,
    importer::{ImportSummary, ImporterRegistry, run_import_batch_with_progress},
    resource_db::{AssetId, ResourceDatabase},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::AtomicBool,
        mpsc::{Sender, channel},
    },
    thread,
};

impl EditorApp {
    /// 启动导入游戏项目（后台执行，非阻塞）。
    pub(crate) fn start_import(&mut self) {
        let path = self.import_path.trim().to_owned();
        if path.is_empty() {
            self.status = "Enter a game project directory first".to_owned();
            return;
        }
        if self.progress.running {
            return;
        }
        let (tx, rx) = channel();
        spawn_import(PathBuf::from(path), tx);
        self.progress
            .start(ProgressKind::ImportProject, "导入游戏项目", rx, None, None);
    }

    /// 每帧轮询通用进度面板；任务完成时应用结果（导入）并落盘发布日志。
    pub(crate) fn poll_progress(&mut self) {
        let finished = self.progress.poll();
        if !finished {
            return;
        }
        self.apply_progress_result();
        // 发布：把输出落盘到 <workspace>/artifacts/builds/release/publish-last.log
        if let Some(dir) = &self.progress.output_dir
            && let Some(parent) = dir.parent()
        {
            let log = parent.join("publish-last.log");
            let _ = std::fs::create_dir_all(parent);
            let _ = std::fs::write(&log, &self.progress.output);
        }
    }

    /// 启动资源后台导入（资源面板刷新后的待导入项），非阻塞。
    pub(crate) fn start_resource_import(&mut self) {
        if self.progress.running || !self.project_loaded {
            return;
        }
        let pending = self.resources.pending_imports().to_vec();
        if pending.is_empty() {
            return;
        }
        let root = self.project.root.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        let (tx, rx) = channel();
        spawn_resource_import(root, pending.clone(), thread_cancel, tx);
        self.resources.record_import_attempts(&pending);
        self.progress.start(
            ProgressKind::ResourceImport,
            "导入资源",
            rx,
            None,
            Some(cancel),
        );
    }

    /// 任务完成：按任务类型应用后台结果。
    fn apply_progress_result(&mut self) {
        if !self.progress.done {
            return;
        }
        match self.progress.kind {
            // 资源导入完成：重新扫描读取最终状态（Ready/ImportFailed）。
            ProgressKind::ResourceImport => {
                if self.progress.success {
                    self.refresh_resources();
                    self.status = format!("资源导入完成：{}", display_path(&self.project.root));
                    // 资源导入成功：自动关闭进度面板（失败保留窗口查看原因），
                    // 与项目导入成功行为一致。
                    self.progress.show = false;
                }
            }
            ProgressKind::ImportProject => {
                let Some(result) = self.progress.result.take() else {
                    return;
                };
                if let Ok(outcome) = result.downcast::<ImportOutcome>() {
                    let outcome = *outcome;
                    self.project = outcome.project;
                    self.document = outcome.document;
                    self.recovery = outcome.recovery;
                    self.recovery_scan = outcome.recovery_scan;
                    self.resources = outcome.resources;
                    // 项目切换：清空旧项目缩略图缓存并让后台解码结果失效。
                    self.thumbnails.clear();
                    self.thumbnail_worker.clear();
                    // 字体图集按项目缓存（project.toml `font` 键）：切换项目后重建。
                    self.scene_font_atlas = None;
                    self.history.clear();
                    self.autosave.reset();
                    self.lua_runtime = None;
                    self.play_session = None;
                    // E03 项目切换：整个文档被替换，清空统一选择模型。
                    self.selection.clear(editor::SelectionSource::Playback);
                    // H06：项目状态文件随新工程根切换；若新工程暂无场景则清空。
                    self.load_hierarchy_collapsed();
                    self.delete_target = crate::DeleteTarget::None;
                    self.project_loaded = true;
                    self.status = format!(
                        "Imported game project: {}",
                        display_path(&self.project.root)
                    );
                    let root = self.project.root.clone();
                    self.record_recent_project(&root);
                    // 导入成功：自动关闭进度面板（失败保留窗口查看原因）。
                    self.progress.show = false;
                }
            }
            ProgressKind::Publish => {}
        }
    }

    /// 记录最近打开的项目（去重、最新在前、最多 5 个），并落盘。
    pub(crate) fn record_recent_project(&mut self, path: &Path) {
        let clean = strip_verbatim_prefix(path).to_string_lossy().into_owned();
        self.recent_projects.retain(|p| p != &clean);
        self.recent_projects.insert(0, clean);
        self.recent_projects.truncate(5);
        save_recent_projects(&self.recent_projects);
    }

    /// 打开最近项目（项目菜单用）：E06 先经未保存确认网关，确认后走后台导入流程。
    pub(crate) fn open_recent_project(&mut self, path: String) {
        self.request_document_change(crate::actions::PendingChange::SwitchProject { path });
    }
}

/// 导入游戏项目的后台结果。
struct ImportOutcome {
    project: ProjectInfo,
    document: ProjectDocument,
    recovery: RecoveryManager,
    recovery_scan: RecoveryScan,
    resources: ResourceBrowserState,
}

/// 后台导入游戏项目：打开项目 + 加载文档 + 扫描资源，结果回传主线程。
fn spawn_import(path: PathBuf, tx: Sender<ProgressEvent>) {
    thread::spawn(move || {
        let _ = tx.send(ProgressEvent::Line(format!(
            "打开项目 {} ...\n",
            display_path(&path)
        )));
        let _ = tx.send(ProgressEvent::Progress(0.1));
        let result = (|| -> Result<ImportOutcome, String> {
            let project =
                open_project(&path).map_err(|error| format_project_error(&path, error))?;
            if !project.root.join("project.toml").is_file() {
                return Err("不是独立游戏项目：缺少 project.toml".to_owned());
            }
            let _ = tx.send(ProgressEvent::Progress(0.3));
            let document = ProjectDocument::open(project.clone());
            let _ = tx.send(ProgressEvent::Progress(0.55));
            let recovery = RecoveryManager::new(project.root.join(".temp"));
            let recovery_scan = recovery.scan().unwrap_or_default();
            let _ = tx.send(ProgressEvent::Progress(0.7));
            let mut resources = ResourceBrowserState::new();
            if let Err(error) = resources.refresh(&project.root) {
                return Err(format!("资源扫描失败：{error}"));
            }
            // 资源扫描是最重的一步，完成后接近收尾。
            let _ = tx.send(ProgressEvent::Progress(0.9));
            Ok(ImportOutcome {
                project,
                document,
                recovery,
                recovery_scan,
                resources,
            })
        })();
        match result {
            Ok(outcome) => {
                let _ = tx.send(ProgressEvent::Line("导入完成。\n".to_owned()));
                let _ = tx.send(ProgressEvent::Finished {
                    success: true,
                    result: Some(Box::new(outcome)),
                });
            }
            Err(error) => {
                let _ = tx.send(ProgressEvent::Line(format!("导入失败：{error}\n")));
                let _ = tx.send(ProgressEvent::Finished {
                    success: false,
                    result: None,
                });
            }
        }
    });
}

/// 后台资源导入：对待导入项跑 importer batch，回写 DB，进度逐行回传。
/// 不依赖前台状态：自行打开 DB/缓存实例，原子写盘防撕裂。
fn spawn_resource_import(
    root: PathBuf,
    items: Vec<(AssetId, PathBuf)>,
    cancel: Arc<AtomicBool>,
    tx: Sender<ProgressEvent>,
) {
    thread::spawn(move || {
        let _ = tx.send(ProgressEvent::Line(format!(
            "导入 {} 个资源...\n",
            items.len()
        )));
        let result = (|| -> Result<ImportSummary, String> {
            let mut database = ResourceDatabase::open(&root)
                .map_err(|error| format!("资源库打开失败：{error}"))?;
            let mut cache =
                ImportCache::open(&root).map_err(|error| format!("导入缓存打开失败：{error}"))?;
            let registry = ImporterRegistry::builtin();
            let items: Vec<(AssetId, PathBuf)> = items
                .into_iter()
                .map(|(id, relative)| (id, root.join(relative)))
                .collect();
            // 按实际导入数量回报进度，驱动确定进度条。
            let tx_progress = tx.clone();
            let summary = run_import_batch_with_progress(
                &root,
                &mut database,
                &mut cache,
                registry,
                &items,
                &cancel,
                |done, total| {
                    let _ = tx_progress.send(ProgressEvent::Progress(done as f32 / total as f32));
                },
            );
            // 导入可能修复/恢复了依赖（如失败的纹理重新就绪）：重新传播失效，
            // 把消费者从 Invalid/Stale 撤回到可消费状态，再写盘。
            database.propagate_invalidation();
            database
                .save()
                .map_err(|error| format!("保存资源数据库失败：{error}"))?;
            Ok(summary)
        })();
        match result {
            Ok(summary) => {
                let _ = tx.send(ProgressEvent::Line(format!(
                    "导入完成：{} 个（导入 {}/复用 {}/失败 {}/跳过 {}），耗时 {}ms\n",
                    summary.total,
                    summary.imported,
                    summary.reused,
                    summary.failed,
                    summary.skipped,
                    summary.elapsed_ms
                )));
                let _ = tx.send(ProgressEvent::Finished {
                    success: true,
                    result: None,
                });
            }
            Err(error) => {
                let _ = tx.send(ProgressEvent::Line(format!("资源导入失败：{error}\n")));
                let _ = tx.send(ProgressEvent::Finished {
                    success: false,
                    result: None,
                });
            }
        }
    });
}

#[cfg(test)]
mod import_tests {
    use super::*;
    use engine::core::logging::LogSink;

    /// 资源导入成功应自动关闭进度面板（与项目导入成功行为一致）；失败保留窗口。
    #[test]
    fn resource_import_success_auto_closes_progress_panel() {
        let project = open_project(engine::core::games_dir().join("examples")).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        // 模拟一个已完成且成功的资源导入。
        app.progress.kind = ProgressKind::ResourceImport;
        app.progress.done = true;
        app.progress.success = true;
        app.progress.show = true;
        app.apply_progress_result();
        assert!(!app.progress.show, "资源导入成功应自动关闭进度面板");
    }

    /// 资源导入失败应保留进度面板，便于查看失败原因。
    #[test]
    fn resource_import_failure_keeps_panel_open() {
        let project = open_project(engine::core::games_dir().join("examples")).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.progress.kind = ProgressKind::ResourceImport;
        app.progress.done = true;
        app.progress.success = false;
        app.progress.show = true;
        app.apply_progress_result();
        assert!(app.progress.show, "资源导入失败应保留进度面板查看原因");
    }

    #[test]
    fn import_spawn_end_to_end() {
        // 复现编辑器「导入游戏项目」后台路径：打开 game 项目并扫描资源。
        let game = engine::core::games_dir().join("examples");
        let (tx, rx) = channel();
        spawn_import(game, tx);
        let mut output = String::new();
        let mut success = false;
        let mut max_progress: f32 = 0.0;
        while let Ok(event) = rx.recv() {
            match event {
                ProgressEvent::Line(text) => output.push_str(&text),
                ProgressEvent::Finished { success: s, result } => {
                    success = s;
                    assert!(result.is_some(), "导入应带回 ImportOutcome 结果");
                    break;
                }
                ProgressEvent::Progress(value) => {
                    assert!((0.0..=1.0).contains(&value), "进度应在 0..=1：{value}");
                    max_progress = max_progress.max(value);
                }
            }
        }
        assert!(success, "导入应成功；输出：\n{output}");
        assert!(
            max_progress >= 0.9,
            "导入应按步骤回报真实进度；max={max_progress}"
        );
    }
}
