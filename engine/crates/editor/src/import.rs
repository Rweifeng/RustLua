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
        // 后台刷新在途时暂停新导入：`pending_imports` 来自上一次落地的刷新，
        // 在途刷新落地后会整体替换状态（含待导入项）。此刻起导入只会重复上一轮
        // 队列（导入完成后同帧弹窗立刻重开，进度条反复停在 100% 进行中）。
        if self.resource_refresh_job.is_some() {
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
                    self.resource_tree_cache = None;
                    // 项目切换：清空旧项目缩略图缓存并让后台解码结果失效。
                    self.thumbnails.clear();
                    self.thumbnail_worker.clear();
                    // 源 Prefab 缓存按项目根解析出的绝对路径缓存，跨项目清空避免残留。
                    self.prefab_source_cache.clear();
                    // 字体图集按项目缓存（project.toml `font` 键）：切换项目后重建。
                    self.scene_font_atlas = None;
                    self.scene_vector_atlas = None;
                    // Scene View renderer 内部缓存项目 root；跨项目必须重建，
                    // 否则相对纹理路径会解析到上一个项目。
                    self.reset_scene_render_resources();
                    // 自定义字体缓存按项目路径缓存，同样随项目切换清空。
                    self.custom_font_atlases.lock().unwrap().clear();
                    self.custom_vector_atlases.lock().unwrap().clear();
                    // 项目设置缓冲按项目读取，切换后重置为默认（下次打开再读取）。
                    self.project_settings = Default::default();
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

/// 从 panic payload 里提取可读消息。
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "未知原因".to_owned()
    }
}

/// 后台导入游戏项目：打开项目 + 加载文档 + 扫描资源，结果回传主线程。
///
/// worker 用 `catch_unwind` 兜底：线程一旦 panic（此前只能静默死亡），也要回传
/// `Finished{success:false}`，否则"导入游戏项目"进度面板会永远停在"进行中"。
fn spawn_import(path: PathBuf, tx: Sender<ProgressEvent>) {
    thread::spawn(move || {
        let _ = tx.send(ProgressEvent::Line(format!(
            "打开项目 {} ...\n",
            display_path(&path)
        )));
        let _ = tx.send(ProgressEvent::Progress(0.1));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> Result<ImportOutcome, String> {
                let project =
                    open_project(&path).map_err(|error| format_project_error(&path, error))?;
                if !project.root.join("project.toml").is_file() {
                    return Err("不是独立游戏项目：缺少 project.toml".to_owned());
                }
                let _ = tx.send(ProgressEvent::Progress(0.3));
                let document = ProjectDocument::open(project.clone());
                let _ = tx.send(ProgressEvent::Progress(0.55));
                let recovery = RecoveryManager::new(project.root.join(".temp"));
                // 恢复快照扫描失败要**说出来**：静默当作"没有快照"会让用户以为编辑内容
                // 已经没了，也让"恢复目录不可读"这类环境问题查不出来。
                let recovery_scan = match recovery.scan() {
                    Ok(scan) => scan,
                    Err(error) => {
                        let _ = tx.send(ProgressEvent::Line(format!(
                            "警告：崩溃恢复快照扫描失败（{error}）；本次导入不会列出待恢复内容。\n"
                        )));
                        RecoveryScan::default()
                    }
                };
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
            },
        ));
        match result {
            Ok(Ok(outcome)) => {
                let _ = tx.send(ProgressEvent::Line("导入完成。\n".to_owned()));
                let _ = tx.send(ProgressEvent::Finished {
                    success: true,
                    result: Some(Box::new(outcome)),
                });
            }
            Ok(Err(error)) => {
                let _ = tx.send(ProgressEvent::Line(format!("导入失败：{error}\n")));
                let _ = tx.send(ProgressEvent::Finished {
                    success: false,
                    result: None,
                });
            }
            Err(panic) => {
                let message = panic_message(&*panic);
                let _ = tx.send(ProgressEvent::Line(format!(
                    "导入线程异常退出：{message}\n"
                )));
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
/// worker 用 `catch_unwind` 兜底：线程一旦 panic（此前只能静默死亡），也要回传
/// `Finished{success:false}`，否则"导入资源"进度面板会永远停在"进行中"。
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
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> Result<ImportSummary, String> {
                let mut database = ResourceDatabase::open(&root)
                    .map_err(|error| format!("资源库打开失败：{error}"))?;
                let mut cache = ImportCache::open(&root)
                    .map_err(|error| format!("导入缓存打开失败：{error}"))?;
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
                        let _ =
                            tx_progress.send(ProgressEvent::Progress(done as f32 / total as f32));
                    },
                );
                // 导入可能修复/恢复了依赖（如失败的纹理重新就绪）：重新传播失效，
                // 把消费者从 Invalid/Stale 撤回到可消费状态，再写盘。进度已到 100%，
                // 先告知用户正在收尾，避免"100% 却一直进行中"被误认作卡死。
                let _ = tx.send(ProgressEvent::Line("正在保存资源库…\n".to_owned()));
                database.propagate_invalidation();
                database
                    .save()
                    .map_err(|error| format!("保存资源数据库失败：{error}"))?;
                Ok(summary)
            },
        ));
        match result {
            Ok(Ok(summary)) => {
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
            Ok(Err(error)) => {
                let _ = tx.send(ProgressEvent::Line(format!("资源导入失败：{error}\n")));
                let _ = tx.send(ProgressEvent::Finished {
                    success: false,
                    result: None,
                });
            }
            Err(panic) => {
                let message = panic_message(&*panic);
                let _ = tx.send(ProgressEvent::Line(format!(
                    "资源导入线程异常退出：{message}\n"
                )));
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

    #[test]
    fn project_import_resets_scene_render_resources() {
        // 旧项目样例：sourcehan_test 已随示例重组删除，改用 game_api 样例工程。
        let old_project = open_project(engine::core::games_dir().join("examples").join("game_api"))
            .expect("open old project");
        let new_project = open_project(engine::core::games_dir().join("examples/game_jhyt"))
            .expect("open new project");
        let mut app = EditorApp::new(old_project, None, LogSink::new(64));
        app.scene_screen_cache_key = Some((12, (1280, 720)));

        let document = ProjectDocument::open(new_project.clone());
        let recovery = RecoveryManager::new(new_project.root.join(".temp"));
        let recovery_scan = recovery.scan().unwrap_or_default();
        let resources = ResourceBrowserState::new();
        app.progress.kind = ProgressKind::ImportProject;
        app.progress.done = true;
        app.progress.success = true;
        app.progress.show = true;
        app.progress.result = Some(Box::new(ImportOutcome {
            project: new_project,
            document,
            recovery,
            recovery_scan,
            resources,
        }));

        app.apply_progress_result();
        assert!(
            app.scene_renderer.is_none(),
            "renderer root must be rebuilt"
        );
        assert!(app.scene_texture.is_none());
        assert!(app.scene_screen_texture.is_none());
        assert_eq!(app.scene_screen_cache_key, None);
    }

    /// 资源导入成功应自动关闭进度面板（与项目导入成功行为一致）；失败保留窗口。
    #[test]
    fn resource_import_success_auto_closes_progress_panel() {
        let project =
            open_project(engine::core::games_dir().join("examples").join("game_api")).unwrap();
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
        let project =
            open_project(engine::core::games_dir().join("examples").join("game_api")).unwrap();
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
        let game = engine::core::games_dir().join("examples").join("game_api");
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
                    assert!(
                        result.is_some(),
                        "导入应带回 ImportOutcome 结果；输出：\n{output}"
                    );
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
